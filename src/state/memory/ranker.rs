use crate::state::index_db::IndexDb;

use super::{
    code_awareness::CodeAwarenessSnapshot,
    query::{build_memory_query_terms, fts_match_query},
    types::{
        MemoryRankSignals, MemoryRecallMode, MemoryScope, MemorySearchHit, MemorySearchQuery,
        MemorySourceKind,
    },
};

const RRF_K: f64 = 60.0;
const AUTO_INJECT_STALE_ENTRY_MIN_SCORE: f64 = 0.28;
const AUTO_INJECT_OBSERVATION_MIN_CONFIDENCE: f64 = 0.72;
const STALE_CHECK_BUDGET_PER_RANK_PASS: usize = 6;

pub async fn search_and_rank(
    db: &IndexDb,
    query: MemorySearchQuery,
) -> anyhow::Result<Vec<MemorySearchHit>> {
    let terms = build_memory_query_terms(&query.query);
    let Some(fts) = fts_match_query(&terms) else {
        return Ok(Vec::new());
    };

    let limit = query.limit.max(1).min(50);
    let workspace_root = match query.project_id.as_deref() {
        Some(project_id) => db.workspace_root_for_project(project_id).await?,
        None => None,
    };
    let code_awareness = CodeAwarenessSnapshot::from_prompt(workspace_root, &query.query);
    let mut hits = db.memory_search_candidates_for_ranker(&query, &fts).await?;
    let text_ranks = text_ranks(&hits, &terms);
    let mut stale_check_budget = STALE_CHECK_BUDGET_PER_RANK_PASS;

    for (index, hit) in hits.iter_mut().enumerate() {
        let text_rank = text_ranks[index];
        let code_score = code_awareness.score_refs_with_budget(
            &hit.files,
            &hit.code_refs,
            &mut stale_check_budget,
        );
        hit.stale = code_score.stale;

        let source_boost = source_boost(hit, query.mode);
        let scope_boost = scope_boost(hit.scope, query.scope);
        let confidence_boost = confidence_boost(hit.confidence);
        let strength_boost = strength_boost(hit.source);
        let candidate_order_boost = candidate_order_boost(index);
        let privacy_penalty = privacy_penalty(hit);
        let final_score = text_rank
            + source_boost
            + scope_boost
            + confidence_boost
            + strength_boost
            + candidate_order_boost
            + code_score.working_set_boost
            + code_score.stale_penalty
            + privacy_penalty;
        let final_score = if final_score.is_finite() {
            final_score
        } else {
            f64::NEG_INFINITY
        };

        hit.rank = MemoryRankSignals {
            text_rank,
            scope_boost,
            confidence_boost,
            strength_boost,
            recency_boost: candidate_order_boost,
            working_set_boost: code_score.working_set_boost,
            stale_penalty: code_score.stale_penalty,
            privacy_penalty,
            final_score,
        };
    }

    if query.mode == MemoryRecallMode::AutoInject {
        hits.retain(|hit| {
            !hit.quarantined
                && auto_inject_source_allowed(hit)
                && (!hit.stale
                    || (hit.source == MemorySourceKind::Entry
                        && hit.rank.final_score >= AUTO_INJECT_STALE_ENTRY_MIN_SCORE))
        });
    }

    hits.sort_by(|left, right| {
        right
            .rank
            .final_score
            .total_cmp(&left.rank.final_score)
            .then_with(|| left.source_id.cmp(&right.source_id))
    });
    hits.truncate(limit);
    Ok(hits)
}

fn source_boost(hit: &MemorySearchHit, mode: MemoryRecallMode) -> f64 {
    match (hit.source, mode) {
        (MemorySourceKind::Entry, MemoryRecallMode::AutoInject) => 0.18,
        (MemorySourceKind::Entry, _) => 0.12,
        (MemorySourceKind::Observation, MemoryRecallMode::AutoInject) => -0.08,
        (MemorySourceKind::Observation, _) => 0.02,
    }
}

fn scope_boost(hit_scope: MemoryScope, query_scope: MemoryScope) -> f64 {
    match (hit_scope, query_scope) {
        (MemoryScope::Thread, MemoryScope::Thread) => 0.12,
        (MemoryScope::Project, MemoryScope::Project | MemoryScope::Thread) => 0.08,
        (MemoryScope::Global, MemoryScope::Global) => 0.04,
        (MemoryScope::Global, _) => 0.01,
        _ => 0.0,
    }
}

fn confidence_boost(confidence: f64) -> f64 {
    if !confidence.is_finite() {
        return -0.14;
    }

    ((confidence.clamp(0.0, 1.0) - 0.5) * 0.28).clamp(-0.14, 0.14)
}

fn strength_boost(source: MemorySourceKind) -> f64 {
    match source {
        MemorySourceKind::Entry => 0.14,
        MemorySourceKind::Observation => 0.0,
    }
}

fn candidate_order_boost(index: usize) -> f64 {
    (0.03 - (index as f64 * 0.001)).max(0.0)
}

fn privacy_penalty(hit: &MemorySearchHit) -> f64 {
    if hit.quarantined {
        -1.0
    } else {
        0.0
    }
}

fn auto_inject_source_allowed(hit: &MemorySearchHit) -> bool {
    match hit.source {
        MemorySourceKind::Entry => true,
        MemorySourceKind::Observation => {
            hit.kind == "user_rule"
                && hit.auto_inject_eligible
                && hit.confidence.is_finite()
                && hit.confidence >= AUTO_INJECT_OBSERVATION_MIN_CONFIDENCE
        }
    }
}

fn text_ranks(hits: &[MemorySearchHit], terms: &[String]) -> Vec<f64> {
    let mut scored = hits
        .iter()
        .enumerate()
        .map(|(index, hit)| (index, text_relevance(hit, terms)))
        .collect::<Vec<_>>();
    scored.sort_by(|(left_index, left_score), (right_index, right_score)| {
        right_score.total_cmp(left_score).then_with(|| {
            hits[*left_index]
                .source_id
                .cmp(&hits[*right_index].source_id)
        })
    });

    let mut ranks = vec![0.0; hits.len()];
    for (rank, (index, _)) in scored.into_iter().enumerate() {
        ranks[index] = 1.0 / (RRF_K + rank as f64 + 1.0);
    }
    ranks
}

fn text_relevance(hit: &MemorySearchHit, terms: &[String]) -> f64 {
    let title = hit.title.to_lowercase();
    let body = hit.body.to_lowercase();
    let files = hit.files.join(" ").to_lowercase();
    let concepts = hit.concepts.join(" ").to_lowercase();

    terms
        .iter()
        .map(|term| term.to_lowercase())
        .map(|term| {
            weighted_contains(&title, &term, 3.0)
                + weighted_contains(&body, &term, 2.0)
                + weighted_contains(&files, &term, 2.0)
                + weighted_contains(&concepts, &term, 1.0)
        })
        .sum()
}

fn weighted_contains(haystack: &str, needle: &str, weight: f64) -> f64 {
    if haystack.contains(needle) {
        weight
    } else {
        0.0
    }
}
