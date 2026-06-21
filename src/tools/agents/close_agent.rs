use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::registry::ToolContext;
use crate::runtime::subagent::{AgentControl, CloseAgentRequest};
use crate::tools::{
    ToolCapabilities, ToolHandler, ToolInvocation, ToolModelOutput, ToolOutcome, ToolRuntimeEffect,
    ToolSpec,
};
use crate::types::{ToolResult, ToolStatus};

#[derive(Clone)]
pub struct CloseAgentTool {
    control: Arc<AgentControl>,
}

impl CloseAgentTool {
    pub fn new(control: Arc<AgentControl>) -> Self {
        Self { control }
    }
}

#[async_trait]
impl ToolHandler for CloseAgentTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "close_agent",
            "Close a child agent and release its live runtime resources.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "agent_path": {
                        "type": "string",
                        "description": "Absolute agent path to close, such as /root/research."
                    }
                },
                "required": ["agent_path"]
            }),
        )
        // Internal contract: describes the JSON object this tool returns as its
        // model-facing `content`. See ADR-0042.
        .with_output_schema(json!({
            "type": "object",
            "properties": {
                "parent_thread_id": { "type": "string", "description": "Thread id of the agent that requested the close." },
                "root_thread_id": { "type": "string", "description": "Root thread id of the agent tree." },
                "closed_agents": {
                    "type": "array",
                    "description": "Agents that were closed, including descendants of the target.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "thread_id": { "type": "string", "description": "Thread id of the closed agent." },
                            "agent_path": { "type": "string", "description": "Absolute agent path that was closed." },
                            "depth": { "type": "integer", "description": "Depth below the root agent (root is 0)." }
                        },
                        "required": ["thread_id", "agent_path", "depth"],
                        "additionalProperties": false
                    }
                },
                "status": { "type": "string", "description": "Always \"closed\" on success." }
            },
            "required": ["parent_thread_id", "root_thread_id", "closed_agents", "status"],
            "additionalProperties": false
        }))
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            mutating: false,
            requires_approval: false,
            parallel_safe: false,
        }
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args = match serde_json::from_value::<CloseAgentArgs>(call.arguments.clone()) {
            Ok(args) => args,
            Err(err) => {
                return error_result(call.id, call.name, format!("invalid arguments: {err}"));
            }
        };
        let Some(parent_thread_id) = ctx.thread_id.clone() else {
            return error_result(call.id, call.name, "thread context missing");
        };

        let response = match self
            .control
            .close_agent(CloseAgentRequest {
                parent_thread_id: parent_thread_id.clone(),
                config: ctx.config.clone(),
                agent_path: args.agent_path,
            })
            .await
        {
            Ok(response) => response,
            Err(err) => return error_result(call.id, call.name, err.to_string()),
        };

        let effects =
            response
                .closed_agents
                .iter()
                .map(|closed| ToolRuntimeEffect::SubagentClosed {
                    invocation_id: invocation.invocation_id.clone(),
                    tool_call_id: call.id.clone(),
                    parent_thread_id: parent_thread_id.clone(),
                    closed_thread_id: closed.thread_id.clone(),
                    agent_path: closed.agent_path.clone(),
                });
        let output = json!({
            "parent_thread_id": response.parent_thread_id.as_str(),
            "root_thread_id": response.root_thread_id.as_str(),
            "closed_agents": response.closed_agents,
            "status": "closed"
        });
        ToolOutcome::success(
            call.id.clone(),
            call.name.clone(),
            ToolModelOutput::text(output.to_string()),
        )
        .with_effects(effects)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CloseAgentArgs {
    agent_path: String,
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
    fn close_agent_schema_requires_agent_path() {
        let lifecycle = Arc::new(TestLifecycle);
        let control = AgentControl::new_root(
            ThreadId::new("thread_schema"),
            Arc::downgrade(&(lifecycle as Arc<dyn SubagentLifecycle>)),
        );
        let tool = CloseAgentTool::new(control);
        let schema = tool.spec().to_internal_schema();
        assert_eq!(schema["name"], "close_agent");
        assert_eq!(schema["input_schema"]["required"][0], "agent_path");
    }

    #[test]
    fn close_agent_output_schema_matches_emitted_content() {
        let lifecycle = Arc::new(TestLifecycle);
        let control = AgentControl::new_root(
            ThreadId::new("thread_schema"),
            Arc::downgrade(&(lifecycle as Arc<dyn SubagentLifecycle>)),
        );
        let spec = CloseAgentTool::new(control).spec();
        let output_schema = spec
            .output_schema
            .expect("close_agent declares output_schema");
        // Keys the handler actually emits in the result `content` JSON.
        assert_eq!(
            output_schema["required"],
            json!([
                "parent_thread_id",
                "root_thread_id",
                "closed_agents",
                "status"
            ])
        );
        assert_eq!(
            output_schema["properties"]["closed_agents"]["items"]["required"],
            json!(["thread_id", "agent_path", "depth"])
        );
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
