use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::registry::ToolContext;
use crate::state::memory::MemorySaveInput;
use crate::tools::memory_common::{
    derive_scope, error, memory_api, success_json, MemoryEntryKindArg, MemoryScopeArg,
    MEMORY_TOOL_ACTOR,
};
use crate::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};

#[derive(Clone)]
pub struct MemoryUpdateTool;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemoryUpdateArgs {
    id: String,
    #[serde(default = "default_scope")]
    scope: MemoryScopeArg,
    action: MemoryUpdateAction,
    kind: Option<MemoryEntryKindArg>,
    title: Option<String>,
    content: Option<String>,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    concepts: Vec<String>,
    pinned: Option<bool>,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum MemoryUpdateAction {
    Update,
    Pin,
    Unpin,
    Withdraw,
    Promote,
    Supersede,
    Archive,
}

#[async_trait]
impl ToolHandler for MemoryUpdateTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "memory_update",
            "Pin, unpin, or supersede memory within the current server-derived scope. Active promotion and in-place edits are not available to model actors.",
            serde_json::to_value(schemars::schema_for!(MemoryUpdateArgs)).unwrap(),
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
        let args: MemoryUpdateArgs = match serde_json::from_value(call.arguments.clone()) {
            Ok(args) => args,
            Err(err) => return error(call.id, call.name, err.to_string()),
        };
        let api = match memory_api(ctx) {
            Ok(api) => api,
            Err(err) => return error(call.id, call.name, err),
        };
        if args.scope == MemoryScopeArg::Global {
            return error(
                call.id,
                call.name,
                "memory_update cannot modify global memory; global memory changes are curator-only",
            );
        }

        match args.action {
            MemoryUpdateAction::Pin | MemoryUpdateAction::Unpin => {
                if args.kind.is_some()
                    || args.title.is_some()
                    || args.content.is_some()
                    || !args.files.is_empty()
                    || !args.concepts.is_empty()
                    || args.pinned.is_some()
                {
                    return error(
                        call.id,
                        call.name,
                        "memory_update pin/unpin does not accept edit fields",
                    );
                }
                let derived = match derive_scope(ctx, args.scope.into()).await {
                    Ok(scope) => scope,
                    Err(err) => return error(call.id, call.name, err),
                };
                let pinned = matches!(args.action, MemoryUpdateAction::Pin);
                match api
                    .runtime()
                    .db()
                    .set_memory_entry_pinned_with_scope(
                        &args.id,
                        pinned,
                        MEMORY_TOOL_ACTOR,
                        derived.project_id.as_deref(),
                        derived.thread_id.as_ref(),
                    )
                    .await
                {
                    Ok(entry) => success_json(
                        call.id,
                        call.name,
                        format!(
                            "Memory entry {} {}.",
                            entry.id,
                            if entry.pinned { "pinned" } else { "unpinned" }
                        ),
                        json!({
                            "id": entry.id,
                            "status": entry.status.as_str(),
                            "scope": entry.scope.as_str(),
                            "pinned": entry.pinned
                        }),
                    ),
                    Err(err) => error(call.id, call.name, err.to_string()),
                }
            }
            MemoryUpdateAction::Supersede => {
                let Some(kind) = args.kind else {
                    return error(call.id, call.name, "memory_update supersede requires kind");
                };
                let Some(title) = args.title else {
                    return error(call.id, call.name, "memory_update supersede requires title");
                };
                let Some(content) = args.content else {
                    return error(
                        call.id,
                        call.name,
                        "memory_update supersede requires content",
                    );
                };
                let derived = match derive_scope(ctx, args.scope.into()).await {
                    Ok(scope) => scope,
                    Err(err) => return error(call.id, call.name, err),
                };
                let input = MemorySaveInput {
                    scope: derived.scope,
                    kind: kind.into(),
                    title,
                    content,
                    files: args.files,
                    concepts: args.concepts,
                    source_refs: crate::tools::memory_common::source_refs_for_tool_call(ctx, &call),
                    pinned: args.pinned.unwrap_or(false),
                };
                match api
                    .runtime()
                    .db()
                    .supersede_memory_entry_with_scope(
                        &args.id,
                        input,
                        MEMORY_TOOL_ACTOR,
                        derived.project_id.as_deref(),
                        derived.thread_id.as_ref(),
                    )
                    .await
                {
                    Ok(entry) => success_json(
                        call.id,
                        call.name,
                        format!("Memory entry {} superseded by {}.", args.id, entry.id),
                        json!({
                            "id": entry.id,
                            "status": entry.status.as_str(),
                            "scope": entry.scope.as_str(),
                            "kind": entry.kind.as_str(),
                            "pinned": entry.pinned,
                            "supersedes_id": entry.supersedes_id
                        }),
                    ),
                    Err(err) => error(call.id, call.name, err.to_string()),
                }
            }
            MemoryUpdateAction::Update
            | MemoryUpdateAction::Withdraw
            | MemoryUpdateAction::Promote
            | MemoryUpdateAction::Archive => {
                let action = match args.action {
                    MemoryUpdateAction::Update => "update",
                    MemoryUpdateAction::Withdraw => "withdraw",
                    MemoryUpdateAction::Promote => "promote",
                    MemoryUpdateAction::Archive => "archive",
                    MemoryUpdateAction::Pin
                    | MemoryUpdateAction::Unpin
                    | MemoryUpdateAction::Supersede => unreachable!(),
                };
                error(
                    call.id,
                    call.name,
                    format!(
                        "memory_update action {action:?} for {} is unsupported for model actors; active promotion and in-place edits require curation",
                        args.id
                    ),
                )
            }
        }
    }
}

fn default_scope() -> MemoryScopeArg {
    MemoryScopeArg::Project
}
