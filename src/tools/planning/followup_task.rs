use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use crate::registry::ToolContext;
use crate::runtime::subagent::AgentControl;
use crate::tools::send_message::handle_message_tool;
use crate::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};

#[derive(Clone)]
pub struct FollowupTaskTool {
    control: Arc<AgentControl>,
}

impl FollowupTaskTool {
    pub fn new(control: Arc<AgentControl>) -> Self {
        Self { control }
    }
}

#[async_trait]
impl ToolHandler for FollowupTaskTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "followup_task",
            "Send a follow-up message to another live agent and start it if idle.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "recipient_path": {
                        "type": "string",
                        "description": "Absolute recipient agent path, such as /root/research."
                    },
                    "message": {
                        "type": "string",
                        "description": "Message content to deliver before the target follow-up turn."
                    }
                },
                "required": ["recipient_path", "message"]
            }),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            mutating: false,
            requires_approval: false,
            parallel_safe: false,
        }
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        handle_message_tool(self.control.clone(), invocation, ctx, true).await
    }
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
    fn followup_task_schema_requires_recipient_and_message() {
        let lifecycle = Arc::new(TestLifecycle);
        let control = AgentControl::new_root(
            ThreadId::new("thread_schema"),
            Arc::downgrade(&(lifecycle as Arc<dyn SubagentLifecycle>)),
        );
        let tool = FollowupTaskTool::new(control);
        let schema = tool.spec().to_internal_schema();
        assert_eq!(schema["name"], "followup_task");
        assert_eq!(schema["input_schema"]["required"][0], "recipient_path");
        assert_eq!(schema["input_schema"]["required"][1], "message");
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
