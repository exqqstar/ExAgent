use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::registry::ToolContext;
use crate::state::memory::MemorySaveInput;
use crate::tools::memory_common::{
    derive_scope, error, memory_api, source_refs_for_tool_call, success_json, MemoryEntryKindArg,
    MemoryScopeArg, MEMORY_TOOL_ACTOR,
};
use crate::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};

#[derive(Clone)]
pub struct MemorySaveTool;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemorySaveArgs {
    #[serde(default = "default_scope")]
    scope: MemoryScopeArg,
    kind: MemoryEntryKindArg,
    title: String,
    content: String,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    concepts: Vec<String>,
}

#[async_trait]
impl ToolHandler for MemorySaveTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "memory_save",
            "Propose one rare long-term memory candidate for human curation using server-derived scope. Use for durable user rules/preferences, architectural decisions with their why, and hard-won gotchas. Do not use for transient task state or facts that grep, git, LSP, or the current files can re-derive.",
            serde_json::to_value(schemars::schema_for!(MemorySaveArgs)).unwrap(),
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
        let args: MemorySaveArgs = match serde_json::from_value(call.arguments.clone()) {
            Ok(args) => args,
            Err(err) => return error(call.id, call.name, err.to_string()),
        };
        if args.scope == MemoryScopeArg::Global {
            return error(
                call.id,
                call.name,
                "memory_save cannot create global memory; global promotion is curator-only",
            );
        }
        let api = match memory_api(ctx) {
            Ok(api) => api,
            Err(err) => return error(call.id, call.name, err),
        };
        let derived = match derive_scope(ctx, args.scope.into()).await {
            Ok(scope) => scope,
            Err(err) => return error(call.id, call.name, err),
        };
        let input = MemorySaveInput {
            scope: derived.scope,
            kind: args.kind.into(),
            title: args.title,
            content: args.content,
            files: args.files,
            concepts: args.concepts,
            source_refs: source_refs_for_tool_call(ctx, &call),
            pinned: false,
        };

        match api
            .runtime()
            .db()
            .find_duplicate_memory_candidate_or_entry(
                derived.project_id.as_deref(),
                derived.thread_id.as_ref(),
                &input,
            )
            .await
        {
            Ok(Some(existing)) => {
                return success_json(
                    call.id,
                    call.name,
                    format!(
                        "Memory candidate skipped; {} is already known.",
                        existing.id
                    ),
                    json!({
                        "status": "skipped",
                        "duplicate_of": existing.id,
                        "scope": existing.scope.as_str(),
                        "kind": existing.kind.as_str(),
                        "pending_curation": false
                    }),
                );
            }
            Ok(None) => {}
            Err(err) => return error(call.id, call.name, err.to_string()),
        }

        match api
            .runtime()
            .db()
            .propose_memory_candidate(
                derived.project_id.as_deref(),
                derived.thread_id.as_ref(),
                input,
                MEMORY_TOOL_ACTOR,
            )
            .await
        {
            Ok(candidate) => success_json(
                call.id,
                call.name,
                format!("Memory candidate {} is pending curation.", candidate.id),
                json!({
                    "id": candidate.id,
                    "status": candidate.status.as_str(),
                    "scope": candidate.scope.as_str(),
                    "kind": candidate.kind.as_str(),
                    "pending_curation": true
                }),
            ),
            Err(err) => error(call.id, call.name, err.to_string()),
        }
    }
}

fn default_scope() -> MemoryScopeArg {
    MemoryScopeArg::Project
}
