use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::registry::ToolContext;
use crate::tools::memory_common::{
    error, format_candidates, memory_api, search_query, success_json, MemoryScopeArg,
};
use crate::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};

#[derive(Clone)]
pub struct MemoryListTool;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemoryListArgs {
    #[serde(default)]
    query: String,
    scope: Option<MemoryScopeArg>,
    limit: Option<usize>,
}

#[async_trait]
impl ToolHandler for MemoryListTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "memory_list",
            "List memory candidates for curation/debug using server-derived scope.",
            serde_json::to_value(schemars::schema_for!(MemoryListArgs)).unwrap(),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_only()
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args: MemoryListArgs = match serde_json::from_value(call.arguments) {
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
                .map(crate::state::memory::MemoryScope::from)
                .unwrap_or(crate::state::memory::MemoryScope::Project),
            args.limit,
        )
        .await
        {
            Ok(query) => query,
            Err(err) => return error(call.id, call.name, err),
        };

        match api.runtime().db().list_memory_candidates(&query).await {
            Ok(candidates) => success_json(
                call.id,
                call.name,
                format_candidates(&candidates),
                json!({
                    "candidates": candidates,
                }),
            ),
            Err(err) => error(call.id, call.name, err.to_string()),
        }
    }
}
