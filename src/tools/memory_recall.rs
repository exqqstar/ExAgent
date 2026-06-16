use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::registry::ToolContext;
use crate::state::memory::MemoryScope;
use crate::tools::memory_common::{
    error, format_hits, memory_api, search_query, success_json, MemoryScopeArg,
};
use crate::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};

#[derive(Clone)]
pub struct MemoryRecallTool;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemoryRecallArgs {
    query: String,
    scope: Option<MemoryScopeArg>,
    limit: Option<usize>,
}

#[async_trait]
impl ToolHandler for MemoryRecallTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "memory_recall",
            "Search curated project, thread, or global memory using server-derived scope.",
            serde_json::to_value(schemars::schema_for!(MemoryRecallArgs)).unwrap(),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_only()
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args: MemoryRecallArgs = match serde_json::from_value(call.arguments) {
            Ok(args) => args,
            Err(err) => return error(call.id, call.name, err.to_string()),
        };
        let api = match memory_api(ctx) {
            Ok(api) => api,
            Err(err) => return error(call.id, call.name, err),
        };
        let query = match search_query(
            ctx,
            args.query,
            args.scope
                .map(MemoryScope::from)
                .unwrap_or(MemoryScope::Project),
            args.limit,
        )
        .await
        {
            Ok(query) => query,
            Err(err) => return error(call.id, call.name, err),
        };

        match api.runtime().db().search_memory(query).await {
            Ok(hits) => {
                let content = format_hits(&hits, ctx.config.memory_tool_context_max_chars);
                success_json(
                    call.id,
                    call.name,
                    content,
                    json!({
                        "hits": hits,
                    }),
                )
            }
            Err(err) => error(call.id, call.name, err.to_string()),
        }
    }
}
