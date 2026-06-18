use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::registry::ToolContext;
use crate::runtime::subagent::AgentControl;
use crate::tools::{
    ToolCapabilities, ToolHandler, ToolInvocation, ToolModelOutput, ToolOutcome, ToolSpec,
};
use crate::types::{ToolResult, ToolStatus};

#[derive(Clone)]
pub struct ListAgentsTool {
    control: Arc<AgentControl>,
}

impl ListAgentsTool {
    pub fn new(control: Arc<AgentControl>) -> Self {
        Self { control }
    }
}

#[async_trait]
impl ToolHandler for ListAgentsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "list_agents",
            "List live agents in the current root agent tree.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {},
                "required": []
            }),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            mutating: false,
            requires_approval: false,
            parallel_safe: true,
        }
    }

    async fn handle(&self, invocation: ToolInvocation, _ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        if let Err(err) = serde_json::from_value::<ListAgentsArgs>(call.arguments.clone()) {
            return error_result(call.id, call.name, format!("invalid arguments: {err}"));
        }

        let output = json!({
            "agents": self.control.list_agents(),
        });
        ToolOutcome::success(
            call.id,
            call.name,
            ToolModelOutput::text(output.to_string()),
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListAgentsArgs {}

fn error_result(
    tool_call_id: impl Into<String>,
    tool_name: impl Into<String>,
    message: impl Into<String>,
) -> ToolOutcome {
    ToolOutcome::from_result(ToolResult {
        tool_call_id: tool_call_id.into(),
        tool_name: tool_name.into(),
        status: ToolStatus::Error,
        content: message.into(),
        meta: None,
        parts: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::subagent::{
        CloseAgentResponse, CloseAgentsRequest, DeliverInterAgentMessageRequest,
        SendMessageResponse, SpawnAgentResponse, SpawnCleanChildRequest, SubagentLifecycle,
    };
    use crate::types::ThreadId;

    #[test]
    fn list_agents_schema_has_no_arguments() {
        let lifecycle = Arc::new(TestLifecycle);
        let control = AgentControl::new_root(
            ThreadId::new("thread_schema"),
            Arc::downgrade(&(lifecycle as Arc<dyn SubagentLifecycle>)),
        );
        let tool = ListAgentsTool::new(control);
        let schema = tool.spec().to_internal_schema();
        assert_eq!(schema["name"], "list_agents");
        assert!(schema["input_schema"]["properties"]
            .as_object()
            .expect("properties object")
            .is_empty());
    }

    struct TestLifecycle;

    #[async_trait]
    impl SubagentLifecycle for TestLifecycle {
        async fn spawn_clean_child(
            &self,
            _request: SpawnCleanChildRequest,
            _control: Arc<AgentControl>,
        ) -> anyhow::Result<SpawnAgentResponse> {
            unreachable!("schema test does not call lifecycle")
        }

        async fn close_agents(
            &self,
            _request: CloseAgentsRequest,
        ) -> anyhow::Result<CloseAgentResponse> {
            unreachable!("schema test does not call lifecycle")
        }

        async fn deliver_inter_agent_message(
            &self,
            _request: DeliverInterAgentMessageRequest,
        ) -> anyhow::Result<SendMessageResponse> {
            unreachable!("schema test does not call lifecycle")
        }
    }
}
