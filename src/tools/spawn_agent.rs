use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::config::ThinkingMode;
use crate::registry::ToolContext;
use crate::resolved::ModelRef;
use crate::runtime::agent_profile::{
    profile_for_type, render_spawn_agent_type_description, AgentType,
};
use crate::runtime::subagent::{message_preview, AgentControl, SpawnAgentRequest};
use crate::state::fork_history::ForkTurns;
use crate::tools::{
    ToolCapabilities, ToolHandler, ToolInvocation, ToolModelOutput, ToolOutcome, ToolRuntimeEffect,
    ToolSpec,
};
use crate::types::{ToolResult, ToolStatus};

/// Model-facing description for `spawn_agent`. The when-to-delegate gate lives
/// here, in the tool itself, rather than in a per-turn injected message: this
/// guidance is only visible when the tool is, and the default posture is to do
/// the work yourself rather than delegate.
const SPAWN_AGENT_DESCRIPTION: &str = "Spawn a subagent thread for a well-scoped task. \
The subagent runs on its own thread and its final answer is returned to you when it finishes.

Default to doing the work yourself. Spawning is the exception, not the reflex. Before spawning, \
form a quick plan and decide what you should do locally right now; do that step yourself.

When to spawn:
- The user explicitly asked for subagents, delegation, or parallel work; or
- A concrete, bounded sidecar task can run in parallel without blocking your immediate next step.

When NOT to spawn (do it yourself instead):
- The request is small, read-only, a lookup, or something you can already answer directly. \
\"Describe the tools\", \"what can you do\", and similar questions are never reasons to spawn.
- The task is the immediate blocking step on the critical path — doing it locally keeps things moving.
- Needing more depth, thoroughness, research, or detail is not by itself a reason to spawn.

Rules:
- Do not duplicate work between yourself and a subagent, and do not re-spawn the same task.
- Give each subagent a concrete, self-contained task it can finish and report back on. Spawned \
worker agents execute and report; they cannot spawn their own subagents, so do not design tasks \
that assume further delegation.";

#[derive(Clone)]
pub struct SpawnAgentTool {
    control: Arc<AgentControl>,
}

impl SpawnAgentTool {
    pub fn new(control: Arc<AgentControl>) -> Self {
        Self { control }
    }
}

#[async_trait]
impl ToolHandler for SpawnAgentTool {
    fn spec(&self) -> ToolSpec {
        let agent_type_enum = AgentType::ALL
            .iter()
            .map(AgentType::as_str)
            .collect::<Vec<_>>();
        let agent_type_description = render_spawn_agent_type_description();
        let thinking_mode_enum = ThinkingMode::ALL
            .iter()
            .map(|mode| mode.label())
            .collect::<Vec<_>>();
        ToolSpec::function(
            "spawn_agent",
            SPAWN_AGENT_DESCRIPTION,
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "task_name": {
                        "type": "string",
                        "description": "Short unique task name for this subagent under the current agent path."
                    },
                    "message": {
                        "type": "string",
                        "description": "Initial task prompt for the subagent."
                    },
                    "agent_type": {
                        "type": "string",
                        "enum": agent_type_enum,
                        "description": agent_type_description
                    },
                    "fork_turns": {
                        "type": "string",
                        "pattern": "^(none|all|[1-9][0-9]*)$",
                        "description": "`none` starts a clean child thread. `all` or a positive integer copies filtered parent history into the child."
                    },
                    "model": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "provider_id": { "type": "string" },
                            "model_id": { "type": "string" }
                        },
                        "required": ["provider_id", "model_id"],
                        "description": "Optional model override for the child agent."
                    },
                    "thinking_mode": {
                        "type": "string",
                        "enum": thinking_mode_enum,
                        "description": "Optional reasoning/thinking mode override for the child agent."
                    },
                    "agent_role": {
                        "type": "string",
                        "description": "Optional metadata role label for the child agent."
                    }
                },
                "required": ["task_name", "message"]
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
        let args = match serde_json::from_value::<SpawnAgentArgs>(call.arguments.clone()) {
            Ok(args) => args,
            Err(err) => {
                return error_result(call.id, call.name, format!("invalid arguments: {err}"))
            }
        };
        let agent_type = args.agent_type.unwrap_or(AgentType::Worker);
        let profile = profile_for_type(Some(agent_type));
        let fork_turns = match args.fork_turns.as_deref() {
            Some(value) => match parse_fork_turns(Some(value)) {
                Ok(fork_turns) => fork_turns,
                Err(err) => return error_result(call.id, call.name, err),
            },
            None => profile.default_fork_turns,
        };
        let thinking_mode = args.thinking_mode.or(profile.default_thinking_mode);
        if args.message.trim().is_empty() {
            return error_result(call.id, call.name, "message must not be empty");
        }
        let Some(parent_thread_id) = ctx.thread_id.clone() else {
            return error_result(call.id, call.name, "thread context missing");
        };

        let response = match self
            .control
            .spawn_agent(SpawnAgentRequest {
                parent_thread_id: parent_thread_id.clone(),
                config: ctx.config.clone(),
                task_name: args.task_name,
                message: args.message.clone(),
                agent_type,
                agent_role: clean_optional_string(args.agent_role),
                fork_turns,
                model: args.model,
                thinking_mode,
            })
            .await
        {
            Ok(response) => response,
            Err(err) => return error_result(call.id, call.name, err.to_string()),
        };

        let task_name = response.task_name.clone();
        let output = json!({
            "thread_id": response.thread_id.as_str(),
            "parent_thread_id": response.parent_thread_id.as_str(),
            "root_thread_id": response.root_thread_id.as_str(),
            "task_name": task_name,
            "turn_id": response.turn_id.as_str(),
            "fork_turns": fork_turns.label(),
            "status": "running"
        });
        ToolOutcome::success(
            call.id.clone(),
            call.name.clone(),
            ToolModelOutput::text(output.to_string()),
        )
        .with_effect(ToolRuntimeEffect::SubagentSpawned {
            invocation_id: invocation.invocation_id,
            tool_call_id: call.id,
            parent_thread_id,
            child_thread_id: response.thread_id,
            task_name,
            message_preview: message_preview(&args.message),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnAgentArgs {
    task_name: String,
    message: String,
    #[serde(default)]
    agent_type: Option<AgentType>,
    #[serde(default)]
    fork_turns: Option<String>,
    #[serde(default)]
    model: Option<ModelRef>,
    #[serde(default)]
    thinking_mode: Option<ThinkingMode>,
    #[serde(default)]
    agent_role: Option<String>,
}

fn parse_fork_turns(value: Option<&str>) -> Result<ForkTurns, String> {
    let Some(value) = value else {
        return Ok(ForkTurns::None);
    };
    let value = value.trim();
    if value.eq_ignore_ascii_case("none") {
        return Ok(ForkTurns::None);
    }
    if value.eq_ignore_ascii_case("all") {
        return Ok(ForkTurns::All);
    }
    let count = value
        .parse::<usize>()
        .map_err(|_| "fork_turns must be `none`, `all`, or a positive integer".to_string())?;
    if count == 0 {
        return Err("fork_turns must be a positive integer when numeric".to_string());
    }
    Ok(ForkTurns::Last(count))
}

fn clean_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
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

    #[test]
    fn spawn_agent_schema_allows_clean_and_forked_history() {
        let lifecycle = Arc::new(TestLifecycle);
        let control = AgentControl::new_root(
            crate::types::ThreadId::new("thread_schema"),
            Arc::downgrade(&(lifecycle as Arc<dyn crate::runtime::subagent::SubagentLifecycle>)),
        );
        let tool = SpawnAgentTool::new(control);
        let schema = tool.spec().to_internal_schema();
        assert_eq!(schema["name"], "spawn_agent");
        assert_eq!(
            schema["input_schema"]["properties"]["fork_turns"]["pattern"],
            "^(none|all|[1-9][0-9]*)$"
        );
        assert_eq!(
            schema["input_schema"]["properties"]["model"]["required"][0],
            "provider_id"
        );
        assert_eq!(
            schema["input_schema"]["properties"]["model"]["required"][1],
            "model_id"
        );
        let expected_thinking_modes = ThinkingMode::ALL
            .iter()
            .map(|mode| serde_json::Value::String(mode.label().to_string()))
            .collect::<Vec<_>>();
        assert_eq!(
            schema["input_schema"]["properties"]["thinking_mode"]["enum"]
                .as_array()
                .unwrap(),
            &expected_thinking_modes
        );
        assert_eq!(
            schema["input_schema"]["properties"]["agent_role"]["type"],
            "string"
        );
        let agent_type_schema = &schema["input_schema"]["properties"]["agent_type"];
        let expected_agent_types = AgentType::ALL
            .iter()
            .map(|agent_type| serde_json::Value::String(agent_type.as_str().to_string()))
            .collect::<Vec<_>>();
        assert_eq!(
            agent_type_schema["enum"].as_array().unwrap(),
            &expected_agent_types
        );
        let description = agent_type_schema["description"].as_str().unwrap();
        assert!(description.contains("Available profiles:"));
        assert!(description.contains("explorer (Explorer):"));
        assert!(description.contains("When to spawn:"));
        assert!(description.contains("Defaults: fork_turns=none, thinking_mode=low"));
        assert!(
            !description.to_lowercase().contains("locked"),
            "schema guidance must not advertise unenforced locking"
        );
    }

    #[test]
    fn spawn_agent_description_carries_when_to_delegate_gate() {
        // The when-to-spawn guidance must live in the tool description (it used
        // to be injected as a per-turn prompt message). See ADR-0041.
        let description = SPAWN_AGENT_DESCRIPTION;
        assert!(description.contains("Default to doing the work yourself"));
        assert!(description.contains("When NOT to spawn"));
        assert!(description.contains("cannot spawn their own subagents"));
    }

    #[test]
    fn parse_fork_turns_accepts_clean_all_and_positive_counts() {
        assert_eq!(parse_fork_turns(None).unwrap(), ForkTurns::None);
        assert_eq!(parse_fork_turns(Some("none")).unwrap(), ForkTurns::None);
        assert_eq!(parse_fork_turns(Some("ALL")).unwrap(), ForkTurns::All);
        assert_eq!(parse_fork_turns(Some("2")).unwrap(), ForkTurns::Last(2));
    }

    #[test]
    fn parse_fork_turns_rejects_invalid_counts() {
        assert!(parse_fork_turns(Some("0")).is_err());
        assert!(parse_fork_turns(Some("")).is_err());
        assert!(parse_fork_turns(Some("-1")).is_err());
        assert!(parse_fork_turns(Some("last")).is_err());
    }

    struct TestLifecycle;

    #[async_trait]
    impl crate::runtime::subagent::SubagentLifecycle for TestLifecycle {
        async fn spawn_clean_child(
            &self,
            _request: crate::runtime::subagent::SpawnCleanChildRequest,
            _control: Arc<AgentControl>,
        ) -> anyhow::Result<crate::runtime::subagent::SpawnAgentResponse> {
            unreachable!("schema test does not call lifecycle")
        }

        async fn close_agents(
            &self,
            _request: crate::runtime::subagent::CloseAgentsRequest,
        ) -> anyhow::Result<crate::runtime::subagent::CloseAgentResponse> {
            unreachable!("schema test does not call lifecycle")
        }

        async fn deliver_inter_agent_message(
            &self,
            _request: crate::runtime::subagent::DeliverInterAgentMessageRequest,
        ) -> anyhow::Result<crate::runtime::subagent::SendMessageResponse> {
            unreachable!("schema test does not call lifecycle")
        }
    }
}
