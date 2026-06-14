use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::registry::ToolContext;
use crate::state::memory::MemoryScope;
use crate::tools::memory_common::{
    derive_scope, error, memory_api, success_json, MemoryScopeArg, MEMORY_TOOL_ACTOR,
};
use crate::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};

#[derive(Clone)]
pub struct MemoryForgetTool;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemoryForgetArgs {
    id: String,
    scope: Option<MemoryScopeArg>,
}

#[async_trait]
impl ToolHandler for MemoryForgetTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "memory_forget",
            "Forget a memory entry by id.",
            serde_json::to_value(schemars::schema_for!(MemoryForgetArgs)).unwrap(),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            mutating: true,
            requires_approval: false,
            parallel_safe: false,
        }
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args: MemoryForgetArgs = match serde_json::from_value(call.arguments) {
            Ok(args) => args,
            Err(err) => return error(call.id, call.name, err.to_string()),
        };
        let api = match memory_api(ctx) {
            Ok(api) => api,
            Err(err) => return error(call.id, call.name, err),
        };
        let scope = args
            .scope
            .map(MemoryScope::from)
            .unwrap_or(MemoryScope::Project);
        if scope == MemoryScope::Global {
            return error(
                call.id,
                call.name,
                "memory_forget cannot delete global memory; global memory changes are curator-only",
            );
        }
        let derived = match derive_scope(ctx, scope).await {
            Ok(scope) => scope,
            Err(err) => return error(call.id, call.name, err),
        };

        match api
            .runtime()
            .db()
            .forget_memory_entry_with_scope(
                &args.id,
                MEMORY_TOOL_ACTOR,
                derived.project_id.as_deref(),
                derived.thread_id.as_ref(),
            )
            .await
        {
            Ok(()) => success_json(
                call.id,
                call.name,
                format!("Memory entry {} forgotten.", args.id),
                json!({
                    "id": args.id,
                    "status": "deleted"
                }),
            ),
            Err(err) => error(call.id, call.name, err.to_string()),
        }
    }
}
