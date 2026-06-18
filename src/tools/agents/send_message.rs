use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::registry::ToolContext;
use crate::runtime::subagent::{AgentControl, SendMessageRequest};
use crate::tools::{
    ToolCapabilities, ToolHandler, ToolInvocation, ToolModelOutput, ToolOutcome, ToolRuntimeEffect,
    ToolSpec,
};
use crate::types::{ToolResult, ToolStatus};

#[derive(Clone)]
pub struct SendMessageTool {
    control: Arc<AgentControl>,
}

impl SendMessageTool {
    pub fn new(control: Arc<AgentControl>) -> Self {
        Self { control }
    }
}

#[async_trait]
impl ToolHandler for SendMessageTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "send_message",
            "Send a message to another live agent without starting a target turn.",
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
                        "description": "Message content to deliver to the recipient mailbox."
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
        handle_message_tool(self.control.clone(), invocation, ctx, false).await
    }
}

pub(crate) async fn handle_message_tool(
    control: Arc<AgentControl>,
    invocation: ToolInvocation,
    ctx: &ToolContext,
    followup: bool,
) -> ToolOutcome {
    let call = invocation.call;
    let args = match serde_json::from_value::<MessageArgs>(call.arguments.clone()) {
        Ok(args) => args,
        Err(err) => return error_result(call.id, call.name, format!("invalid arguments: {err}")),
    };
    let Some(author_thread_id) = ctx.thread_id.clone() else {
        return error_result(call.id, call.name, "thread context missing");
    };

    let response = match control
        .send_message(SendMessageRequest {
            author_thread_id: author_thread_id.clone(),
            config: ctx.config.clone(),
            recipient_path: args.recipient_path,
            message: args.message,
            source_turn_id: ctx.turn_id.clone(),
            followup,
        })
        .await
    {
        Ok(response) => response,
        Err(err) => return error_result(call.id, call.name, err.to_string()),
    };

    let interaction = if followup {
        "followup_task"
    } else {
        "send_message"
    };
    let author_path = response.mail.author_path.clone();
    let recipient_path = response.mail.recipient_path.clone();
    let recipient_thread_id = response.mail.recipient_thread_id.clone();
    let content_preview = crate::runtime::subagent::message_preview(&response.mail.content);
    let output = json!({
        "interaction": interaction,
        "author_thread_id": response.mail.author_thread_id.as_str(),
        "author_path": author_path,
        "recipient_thread_id": recipient_thread_id.as_str(),
        "recipient_path": recipient_path,
        "started_turn_id": response.started_turn_id.as_ref().map(|turn_id| turn_id.as_str()),
        "target_busy": response.target_busy,
        "status": "sent"
    });
    ToolOutcome::success(
        call.id.clone(),
        call.name.clone(),
        ToolModelOutput::text(output.to_string()),
    )
    .with_effect(ToolRuntimeEffect::InterAgentMessageSent {
        invocation_id: invocation.invocation_id,
        tool_call_id: call.id,
        author_thread_id,
        recipient_thread_id,
        author_path,
        recipient_path,
        content_preview,
        followup: response.followup,
        started_turn_id: response.started_turn_id,
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageArgs {
    recipient_path: String,
    message: String,
}

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
    fn send_message_schema_requires_recipient_and_message() {
        let lifecycle = Arc::new(TestLifecycle);
        let control = AgentControl::new_root(
            ThreadId::new("thread_schema"),
            Arc::downgrade(&(lifecycle as Arc<dyn SubagentLifecycle>)),
        );
        let tool = SendMessageTool::new(control);
        let schema = tool.spec().to_internal_schema();
        assert_eq!(schema["name"], "send_message");
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
