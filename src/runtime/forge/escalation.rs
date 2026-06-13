use anyhow::Result;

use crate::runtime::forge::open_questions::{OpenQuestion, OpenQuestionStore};
use crate::runtime::forge::review::{
    ReviewRejectCategory, ReviewStatus, ReviewStore, ReviewTicket,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForgeReviewGuidance {
    pub(crate) ticket_id: String,
    pub(crate) stuck_count: usize,
    pub(crate) reject_category: ReviewRejectCategory,
    pub(crate) findings: Option<String>,
    pub(crate) open_questions: Vec<OpenQuestion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForgeEscalationDecision {
    None,
    ActiveGuidance { key: String, content: String },
    PauseForQuestions { key: String, content: String },
    BlockedExternal { key: String, content: String },
}

pub(crate) async fn decision_for_goal(
    review_store: &ReviewStore,
    question_store: &OpenQuestionStore,
    goal_id: &str,
) -> Result<ForgeEscalationDecision> {
    let Some(ticket) = review_store.latest_ticket(goal_id).await? else {
        return Ok(ForgeEscalationDecision::None);
    };
    if ticket.status != ReviewStatus::Rejected {
        return Ok(ForgeEscalationDecision::None);
    }
    let open_questions = question_store.unresolved_for_goal(goal_id).await?;
    let guidance = guidance_from_ticket(
        &ticket,
        review_store.consecutive_stuck_count(goal_id).await?,
        open_questions,
    );
    let key = format!(
        "forge_review:{}:{}:{}",
        goal_id, guidance.ticket_id, guidance.stuck_count
    );
    let content = guidance_prompt(&guidance);
    Ok(match guidance.reject_category {
        ReviewRejectCategory::ExternalBlocker => {
            ForgeEscalationDecision::BlockedExternal { key, content }
        }
        ReviewRejectCategory::NeedsUser if !guidance.open_questions.is_empty() => {
            ForgeEscalationDecision::PauseForQuestions { key, content }
        }
        ReviewRejectCategory::NeedsUser | ReviewRejectCategory::RetriableGap => {
            ForgeEscalationDecision::ActiveGuidance { key, content }
        }
    })
}

fn guidance_from_ticket(
    ticket: &ReviewTicket,
    stuck_count: usize,
    open_questions: Vec<OpenQuestion>,
) -> ForgeReviewGuidance {
    ForgeReviewGuidance {
        ticket_id: ticket.ticket_id.clone(),
        stuck_count,
        reject_category: ticket
            .reject_category
            .unwrap_or(ReviewRejectCategory::RetriableGap),
        findings: ticket.findings.clone(),
        open_questions,
    }
}

pub(crate) fn guidance_prompt(guidance: &ForgeReviewGuidance) -> String {
    let mut parts = vec![format!(
        "Forge reviewer rejected review ticket {}.",
        guidance.ticket_id
    )];
    parts.push(format!(
        "Reject category: {}.",
        guidance.reject_category.as_str()
    ));
    if let Some(findings) = guidance.findings.as_deref() {
        parts.push(format!("Reviewer findings: {findings}"));
    }
    if guidance.stuck_count >= 2 {
        parts.push(
            "No progress detected across repeated review rejects with the same workspace hash; change approach, inspect the failure from a different angle, and spawn an appropriate planner or reviewer before retrying."
                .to_string(),
        );
    }
    if !guidance.open_questions.is_empty() {
        let questions = guidance
            .open_questions
            .iter()
            .map(|question| format!("{} ({})", question.question, question.blocks_what))
            .collect::<Vec<_>>()
            .join("; ");
        parts.push(format!("Open questions blocking completion: {questions}."));
    }
    match guidance.reject_category {
        ReviewRejectCategory::NeedsUser => parts.push(
            "Use defer_question for user-owned decisions and continue any independent work that remains."
                .to_string(),
        ),
        ReviewRejectCategory::ExternalBlocker => parts.push(
            "The remaining blocker is external; do not spin on retries without a changed external condition."
                .to_string(),
        ),
        ReviewRejectCategory::RetriableGap => {}
    }
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::forge::open_questions::OpenQuestion;
    use crate::runtime::forge::review::ReviewRejectCategory;
    use crate::types::ThreadId;

    #[test]
    fn retriable_reject_with_progress_uses_findings_without_no_progress_guidance() {
        let guidance = ForgeReviewGuidance {
            ticket_id: "rev_1".to_string(),
            stuck_count: 1,
            reject_category: ReviewRejectCategory::RetriableGap,
            findings: Some("missing regression test".to_string()),
            open_questions: Vec::new(),
        };

        let prompt = guidance_prompt(&guidance);

        assert!(prompt.contains("missing regression test"));
        assert!(!prompt.contains("change approach"));
    }

    #[test]
    fn repeated_rejects_with_same_hash_add_no_progress_guidance() {
        let guidance = ForgeReviewGuidance {
            ticket_id: "rev_2".to_string(),
            stuck_count: 2,
            reject_category: ReviewRejectCategory::RetriableGap,
            findings: Some("same gap persists".to_string()),
            open_questions: Vec::new(),
        };

        let prompt = guidance_prompt(&guidance);

        assert!(prompt.contains("same gap persists"));
        assert!(prompt.contains("change approach"));
        assert!(prompt.contains("spawn"));
    }

    #[test]
    fn needs_user_guidance_names_open_questions() {
        let guidance = ForgeReviewGuidance {
            ticket_id: "rev_3".to_string(),
            stuck_count: 1,
            reject_category: ReviewRejectCategory::NeedsUser,
            findings: Some("need user choice".to_string()),
            open_questions: vec![OpenQuestion {
                question_id: "oq_1".to_string(),
                thread_id: ThreadId::new("thread_escalation"),
                goal_id: "goal_1".to_string(),
                question: "Which customer segment is first?".to_string(),
                blocks_what: "Rollout targeting".to_string(),
                status: crate::runtime::forge::open_questions::OpenQuestionStatus::Open,
                answer: None,
                created_at_ms: 1,
                updated_at_ms: 1,
            }],
        };

        let prompt = guidance_prompt(&guidance);

        assert!(prompt.contains("Which customer segment is first?"));
        assert!(prompt.contains("Rollout targeting"));
    }
}
