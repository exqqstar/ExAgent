use exagent::state::memory::query::{
    build_memory_query_terms, fts_match_query, should_auto_recall,
};

#[test]
fn skips_short_continuation_prompts() {
    for prompt in ["继续", "跑测试", "fix it"] {
        assert!(
            !should_auto_recall(prompt),
            "{prompt:?} should skip auto recall"
        );
    }
}

#[test]
fn recalls_for_memory_and_history_intent() {
    for prompt in [
        "之前我们怎么约定架构规则的？",
        "上次的规则是什么",
        "What decision did we make earlier about memory injection?",
    ] {
        assert!(
            should_auto_recall(prompt),
            "{prompt:?} should trigger auto recall"
        );
    }
}

#[test]
fn query_terms_prefer_paths_symbols_and_decision_words() {
    let terms = build_memory_query_terms(
        "之前 src/runtime/context.rs 的 ContextManager memory injection 决定是什么？",
    );

    assert!(terms.contains(&"src/runtime/context.rs".to_string()));
    assert!(terms.contains(&"ContextManager".to_string()));
    assert!(terms.contains(&"memory".to_string()));
    assert!(terms.contains(&"injection".to_string()));
    assert!(terms.len() <= 12);
}

#[test]
fn query_terms_cap_keeps_late_preferred_terms() {
    let terms = build_memory_query_terms(
        "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima \
         src/state/memory/query.rs MemoryQuery decision",
    );

    assert!(terms.contains(&"src/state/memory/query.rs".to_string()));
    assert!(terms.contains(&"MemoryQuery".to_string()));
    assert!(terms.contains(&"decision".to_string()));
    assert!(terms.len() <= 12);
}

#[test]
fn fts_match_query_quotes_and_escapes_terms() {
    let query = fts_match_query(&[
        "memory".to_string(),
        "src/runtime/context.rs".to_string(),
        r#"decision"quote"#.to_string(),
    ]);

    assert_eq!(
        query.as_deref(),
        Some(r#""memory" OR "src/runtime/context.rs" OR "decision""quote""#)
    );
    assert_eq!(fts_match_query(&[]), None);
}
