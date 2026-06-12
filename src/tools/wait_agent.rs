use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio::time::timeout;

use crate::registry::ToolContext;
use crate::tools::{
    ToolCapabilities, ToolHandler, ToolInvocation, ToolModelOutput, ToolOutcome, ToolSpec,
};
use crate::types::{ToolResult, ToolStatus};

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 300_000;

#[derive(Clone, Copy)]
pub struct WaitAgentTool;

#[async_trait]
impl ToolHandler for WaitAgentTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "wait_agent",
            "Wait for mailbox activity on the current agent.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "timeout_ms": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_TIMEOUT_MS,
                        "description": "Maximum time to wait for mailbox activity in milliseconds."
                    }
                },
                "required": []
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
        let call = invocation.call;
        let args = match serde_json::from_value::<WaitAgentArgs>(call.arguments.clone()) {
            Ok(args) => args,
            Err(err) => {
                return error_result(call.id, call.name, format!("invalid arguments: {err}"));
            }
        };

        let Some(inbox) = ctx.inbox.clone() else {
            return error_result(call.id, call.name, "mailbox context missing");
        };
        let mut mailbox_watch = inbox.subscribe_mailbox().await;
        let timeout_ms = args
            .timeout_ms
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .clamp(1, MAX_TIMEOUT_MS);

        let result = match timeout(Duration::from_millis(timeout_ms), mailbox_watch.changed()).await
        {
            Ok(Ok(())) => json!({
                "message": "Wait completed.",
                "timed_out": false
            }),
            Ok(Err(_)) => {
                return error_result(call.id, call.name, "mailbox is closed");
            }
            Err(_) => json!({
                "message": "Wait timed out.",
                "timed_out": true
            }),
        };

        ToolOutcome::success(
            call.id,
            call.name,
            ToolModelOutput::text(result.to_string()),
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitAgentArgs {
    #[serde(default)]
    timeout_ms: Option<u64>,
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
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::Value;
    use tokio::time::{sleep, Duration};

    use super::*;
    use crate::config::AgentConfig;
    use crate::exec_session::ExecSessionManager;
    use crate::policy::PolicyManager;
    use crate::runtime::subagent::InterAgentCommunication;
    use crate::runtime::thread_session::ThreadInbox;
    use crate::types::{ThreadId, ToolCall, TurnId};

    fn tool_context(inbox: Option<Arc<ThreadInbox>>) -> ToolContext {
        ToolContext {
            config: AgentConfig::default(),
            thread_id: None,
            turn_id: None,
            tool_invocation_id: None,
            exec_sessions: Arc::new(ExecSessionManager::default()),
            exec_output_sink: None,
            policy: Arc::new(PolicyManager::default()),
            agent_tool_policy: crate::runtime::agent_profile::AgentToolPolicy::all(),
            inbox,
            goal_api: None,
        }
    }

    fn inbox() -> Arc<ThreadInbox> {
        Arc::new(ThreadInbox::new(ThreadId::new("thread_parent")))
    }

    fn mail(content: &str) -> InterAgentCommunication {
        InterAgentCommunication {
            author_thread_id: ThreadId::new("thread_child"),
            author_path: "/root/child".into(),
            recipient_thread_id: ThreadId::new("thread_parent"),
            recipient_path: "/root".into(),
            other_recipients: Vec::new(),
            content: content.into(),
            trigger_turn: false,
            source_turn_id: Some(TurnId::new("turn_child")),
            created_at: "2026-06-12T00:00:00Z".into(),
        }
    }

    fn invocation(arguments: Value) -> ToolInvocation {
        ToolInvocation {
            invocation_id: "inv_wait".into(),
            call: ToolCall {
                id: "call_wait".into(),
                name: "wait_agent".into(),
                arguments,
                thought_signature: None,
            },
        }
    }

    #[test]
    fn wait_agent_schema_exposes_timeout() {
        let schema = WaitAgentTool.spec().to_internal_schema();

        assert_eq!(schema["name"], "wait_agent");
        assert_eq!(schema["input_schema"]["required"], json!([]));
        assert_eq!(
            schema["input_schema"]["properties"]["timeout_ms"]["maximum"],
            MAX_TIMEOUT_MS
        );
    }

    #[tokio::test]
    async fn wait_agent_completes_on_mailbox_activity() {
        let inbox = inbox();
        let ctx = tool_context(Some(inbox.clone()));
        let tool = WaitAgentTool;

        let wait = tokio::spawn(async move {
            tool.handle(
                invocation(json!({
                    "timeout_ms": 1_000
                })),
                &ctx,
            )
            .await
        });
        sleep(Duration::from_millis(10)).await;
        inbox.enqueue(mail("done")).await.expect("enqueue mail");

        let outcome = wait.await.expect("wait task joins");

        assert_eq!(outcome.model_result.status, ToolStatus::Success);
        let output: Value = serde_json::from_str(&outcome.model_result.content).unwrap();
        assert_eq!(output["timed_out"], false);
    }

    #[tokio::test]
    async fn wait_agent_observes_mail_that_arrived_before_subscription() {
        let inbox = inbox();
        inbox
            .enqueue(mail("done before wait"))
            .await
            .expect("enqueue mail");
        let ctx = tool_context(Some(inbox));

        let outcome = WaitAgentTool
            .handle(
                invocation(json!({
                    "timeout_ms": 1_000
                })),
                &ctx,
            )
            .await;

        assert_eq!(outcome.model_result.status, ToolStatus::Success);
        let output: Value = serde_json::from_str(&outcome.model_result.content).unwrap();
        assert_eq!(output["timed_out"], false);
    }

    #[tokio::test]
    async fn wait_agent_reports_timeout() {
        let ctx = tool_context(Some(inbox()));
        let outcome = WaitAgentTool
            .handle(
                invocation(json!({
                    "timeout_ms": 1
                })),
                &ctx,
            )
            .await;

        assert_eq!(outcome.model_result.status, ToolStatus::Success);
        let output: Value = serde_json::from_str(&outcome.model_result.content).unwrap();
        assert_eq!(output["timed_out"], true);
    }

    #[tokio::test]
    async fn wait_agent_errors_without_mailbox_context() {
        let ctx = tool_context(None);
        let outcome = WaitAgentTool.handle(invocation(json!({})), &ctx).await;

        assert_eq!(outcome.model_result.status, ToolStatus::Error);
        assert!(outcome
            .model_result
            .content
            .contains("mailbox context missing"));
    }
}
