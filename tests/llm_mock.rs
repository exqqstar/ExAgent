use exagent::llm::{LlmClient, MockLlm};
use exagent::types::AssistantTurn;

#[tokio::test]
async fn mock_llm_returns_scripted_turns_in_order() {
    let llm = MockLlm::new(vec![
        AssistantTurn {
            text: Some("first".into()),
            tool_calls: vec![],
        },
        AssistantTurn {
            text: Some("second".into()),
            tool_calls: vec![],
        },
    ]);

    let first = llm.complete(&[], &[]).await.unwrap();
    let second = llm.complete(&[], &[]).await.unwrap();

    assert_eq!(first.text.as_deref(), Some("first"));
    assert_eq!(second.text.as_deref(), Some("second"));
}
