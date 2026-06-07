use exagent::llm::{LlmClient, LlmRequestOptions, MockLlm};
use exagent::types::AssistantTurn;

#[tokio::test]
async fn mock_llm_returns_scripted_turns_in_order() {
    let llm = MockLlm::new(vec![
        AssistantTurn {
            text: Some("first".into()),
            tool_calls: vec![],
            reasoning: vec![],
        },
        AssistantTurn {
            text: Some("second".into()),
            tool_calls: vec![],
            reasoning: vec![],
        },
    ]);

    let first = llm
        .complete(&[], &[], &LlmRequestOptions::default())
        .await
        .unwrap();
    let second = llm
        .complete(&[], &[], &LlmRequestOptions::default())
        .await
        .unwrap();

    assert_eq!(first.turn.text.as_deref(), Some("first"));
    assert_eq!(second.turn.text.as_deref(), Some("second"));
    assert_eq!(first.token_usage, None);
    assert_eq!(second.token_usage, None);
}
