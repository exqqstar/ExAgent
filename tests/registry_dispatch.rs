use exagent::registry::ToolRegistry;
use exagent::types::ToolCall;
use serde_json::json;

#[tokio::test]
async fn registry_returns_error_result_for_unknown_tool() {
    let registry = ToolRegistry::new();
    let call = ToolCall {
        id: "call_1".into(),
        name: "does_not_exist".into(),
        arguments: json!({}),
    };

    let result = registry.execute(call, None).await;
    assert_eq!(result.tool_name, "does_not_exist");
    assert_eq!(result.status.as_str(), "error");
    assert!(result.content.contains("Unknown tool"));
}
