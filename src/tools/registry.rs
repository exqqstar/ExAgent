use std::collections::HashMap;
use std::sync::Arc;

use crate::config::AgentConfig;
use crate::exec_session::{ExecOutputEventSink, ExecSessionManager};
use crate::policy::PolicyManager;
use crate::runtime::agent_profile::AgentToolPolicy;
use crate::runtime::goal::GoalToolApi;
use crate::runtime::thread_session::ThreadInbox;
use crate::tools::{ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};
use crate::types::{ThreadId, ToolCall, ToolResult, ToolStatus, TurnId};

#[derive(Clone)]
pub struct ToolContext {
    pub config: AgentConfig,
    pub thread_id: Option<ThreadId>,
    pub turn_id: Option<TurnId>,
    pub tool_invocation_id: Option<String>,
    pub exec_sessions: Arc<ExecSessionManager>,
    pub exec_output_sink: Option<ExecOutputEventSink>,
    pub policy: Arc<PolicyManager>,
    pub agent_tool_policy: AgentToolPolicy,
    pub inbox: Option<Arc<ThreadInbox>>,
    pub goal_api: Option<Arc<GoalToolApi>>,
}

#[derive(Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ToolHandler>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register<T>(&mut self, tool: T)
    where
        T: ToolHandler + 'static,
    {
        self.register_handler(tool);
    }

    pub fn register_handler<T>(&mut self, tool: T)
    where
        T: ToolHandler + 'static,
    {
        self.tools.insert(tool.spec().name, Arc::new(tool));
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|tool| tool.spec()).collect()
    }

    pub(crate) fn handler(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        self.tools.get(name).cloned()
    }

    pub fn schemas(&self) -> Vec<serde_json::Value> {
        self.specs()
            .into_iter()
            .map(|spec| spec.to_internal_schema())
            .collect()
    }

    pub async fn execute_outcome(
        &self,
        invocation: ToolInvocation,
        ctx: &ToolContext,
    ) -> ToolOutcome {
        match self.tools.get(&invocation.call.name) {
            Some(tool) => {
                if !ctx.agent_tool_policy.allows(&invocation.call.name) {
                    return denied_by_agent_profile_outcome(invocation.call);
                }
                let mut handler_ctx = ctx.clone();
                handler_ctx.tool_invocation_id = Some(invocation.invocation_id.clone());
                tool.handle(invocation, &handler_ctx).await
            }
            None => ToolOutcome::from_result(ToolResult {
                tool_call_id: invocation.call.id,
                tool_name: invocation.call.name.clone(),
                status: ToolStatus::Error,
                content: format!("Unknown tool: {}", invocation.call.name),
                meta: None,
            }),
        }
    }

    pub async fn execute(&self, call: ToolCall, ctx: Option<&ToolContext>) -> ToolResult {
        if !self.tools.contains_key(&call.name) {
            return ToolResult {
                tool_call_id: call.id,
                tool_name: call.name.clone(),
                status: ToolStatus::Error,
                content: format!("Unknown tool: {}", call.name),
                meta: None,
            };
        }
        let Some(ctx) = ctx else {
            return ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Error,
                content: "Tool context missing".into(),
                meta: None,
            };
        };
        let invocation = ToolInvocation {
            invocation_id: format!("inv_{}", call.id),
            call,
        };
        self.execute_outcome(invocation, ctx).await.model_result
    }
}

fn denied_by_agent_profile_outcome(call: ToolCall) -> ToolOutcome {
    ToolOutcome::from_result(ToolResult {
        tool_call_id: call.id,
        tool_name: call.name.clone(),
        status: ToolStatus::Error,
        content: format!("Tool denied by agent profile: {}", call.name),
        meta: None,
    })
}
