const SHORT_CHINESE_CONTINUATIONS: &[&str] =
    &["继续", "继续吧", "跑测试", "测试", "修一下", "看下"];
const SHORT_ENGLISH_CONTINUATIONS: &[&str] = &["fix it", "continue", "run tests", "try again"];

const MEMORY_INTENT_MARKERS: &[&str] = &[
    "之前",
    "上次",
    "记得",
    "记忆",
    "规则",
    "偏好",
    "决定",
    "约定",
    "架构",
    "memory",
    "remember",
    "previous",
    "earlier",
    "decided",
    "decision",
    "preference",
    "rule",
    "architecture",
];

const QUERY_STOPWORDS: &[&str] = &[
    "the", "and", "or", "but", "with", "from", "into", "this", "that", "继续", "我们", "这个",
    "里面", "方案",
];

/// Cheap deterministic gate for deciding whether a prompt should trigger memory recall.
pub fn should_auto_recall(prompt: &str) -> bool {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return false;
    }

    if is_short_continuation_prompt(trimmed) {
        return false;
    }

    let lower = trimmed.to_lowercase();
    if MEMORY_INTENT_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return true;
    }

    trimmed.contains('?') || trimmed.contains('？')
}

/// Builds a compact term list for memory FTS queries.
pub fn build_memory_query_terms(prompt: &str) -> Vec<String> {
    let mut terms = Vec::new();

    for raw in prompt.split(is_term_separator) {
        let term = trim_term(raw);
        if !should_keep_query_term(term) {
            continue;
        }

        if !terms.iter().any(|existing| existing == term) {
            terms.push(term.to_string());
        }
    }

    cap_query_terms(terms)
}

/// Converts query terms into a safely quoted SQLite FTS MATCH expression.
pub fn fts_match_query(terms: &[String]) -> Option<String> {
    if terms.is_empty() {
        return None;
    }

    Some(
        terms
            .iter()
            .map(|term| format!(r#""{}""#, term.replace('"', r#""""#)))
            .collect::<Vec<_>>()
            .join(" OR "),
    )
}

fn is_short_continuation_prompt(prompt: &str) -> bool {
    let compact = strip_wrapping_punctuation(prompt)
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    if SHORT_CHINESE_CONTINUATIONS
        .iter()
        .any(|continuation| compact == *continuation)
    {
        return true;
    }

    let normalized = strip_wrapping_punctuation(prompt)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    SHORT_ENGLISH_CONTINUATIONS
        .iter()
        .any(|continuation| normalized == *continuation)
}

fn strip_wrapping_punctuation(input: &str) -> &str {
    input.trim_matches(|ch: char| {
        ch.is_whitespace()
            || matches!(
                ch,
                '.' | ','
                    | ';'
                    | ':'
                    | '!'
                    | '?'
                    | '。'
                    | '，'
                    | '；'
                    | '：'
                    | '！'
                    | '？'
                    | '"'
                    | '\''
                    | '`'
                    | '['
                    | ']'
                    | '('
                    | ')'
                    | '{'
                    | '}'
                    | '<'
                    | '>'
                    | '（'
                    | '）'
                    | '【'
                    | '】'
                    | '《'
                    | '》'
            )
    })
}

fn is_term_separator(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            ',' | ';'
                | ':'
                | '!'
                | '?'
                | '。'
                | '，'
                | '、'
                | '；'
                | '：'
                | '！'
                | '？'
                | '('
                | ')'
                | '{'
                | '}'
                | '<'
                | '>'
                | '（'
                | '）'
        )
}

fn trim_term(term: &str) -> &str {
    term.trim_matches(|ch: char| {
        ch.is_whitespace()
            || matches!(
                ch,
                '"' | '\''
                    | '`'
                    | '['
                    | ']'
                    | '('
                    | ')'
                    | '{'
                    | '}'
                    | '<'
                    | '>'
                    | '“'
                    | '”'
                    | '‘'
                    | '’'
                    | '【'
                    | '】'
                    | '《'
                    | '》'
            )
    })
}

fn should_keep_query_term(term: &str) -> bool {
    if term.chars().count() < 3 {
        return false;
    }

    let lower = term.to_lowercase();
    if QUERY_STOPWORDS.contains(&lower.as_str()) {
        return false;
    }

    let has_path_shape = term.contains('/') || term.contains('.');
    let has_symbol_shape = term.contains('_') || term.chars().any(|ch| ch.is_ascii_uppercase());
    let has_alphanumeric = term.chars().any(|ch| ch.is_alphanumeric());

    has_path_shape || has_symbol_shape || has_alphanumeric
}

fn cap_query_terms(terms: Vec<String>) -> Vec<String> {
    if terms.len() <= 12 {
        return terms;
    }

    let preferred_count = terms
        .iter()
        .filter(|term| is_preferred_query_term(term))
        .count();
    let fallback_budget = 12usize.saturating_sub(preferred_count.min(12));
    let mut fallback_used = 0;
    let mut selected = Vec::new();

    for term in terms {
        if is_preferred_query_term(&term) {
            if selected.len() < 12 {
                selected.push(term);
            }
        } else if fallback_used < fallback_budget {
            fallback_used += 1;
            selected.push(term);
        }

        if selected.len() == 12 && fallback_used == fallback_budget {
            let has_room_for_late_preferred = selected
                .iter()
                .filter(|term| is_preferred_query_term(term))
                .count()
                < preferred_count.min(12);
            if !has_room_for_late_preferred {
                break;
            }
        }
    }

    selected
}

fn is_preferred_query_term(term: &str) -> bool {
    let lower = term.to_lowercase();
    let is_memory_intent_word = MEMORY_INTENT_MARKERS
        .iter()
        .any(|marker| marker.is_ascii() && lower == *marker);

    term.contains('/')
        || term.contains('.')
        || term.contains('_')
        || term.chars().any(|ch| ch.is_ascii_uppercase())
        || is_memory_intent_word
}
