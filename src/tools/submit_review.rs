use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::registry::ToolContext;
use crate::runtime::forge::review::{ReviewRejectCategory, ReviewStore, ReviewVerdict};
use crate::tools::{
    ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolRuntimeEffect, ToolSpec,
};
use crate::types::{ToolResult, ToolStatus};
use crate::workspace_checkpoint::create_checkpoint;

#[derive(Clone)]
pub(crate) struct SubmitReviewTool {
    store: ReviewStore,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SubmitReviewArgs {
    ticket_id: String,
    verdict: SubmitReviewVerdictArg,
    #[serde(default)]
    findings: Option<String>,
    #[serde(default)]
    category: Option<SubmitReviewRejectCategoryArg>,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum SubmitReviewVerdictArg {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum SubmitReviewRejectCategoryArg {
    RetriableGap,
    NeedsUser,
    ExternalBlocker,
}

impl SubmitReviewTool {
    pub(crate) fn new(store: ReviewStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolHandler for SubmitReviewTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "submit_review",
            "Submit the reviewer verdict for an open Forge review ticket.",
            serde_json::to_value(schemars::schema_for!(SubmitReviewArgs)).unwrap(),
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
        let args = match serde_json::from_value::<SubmitReviewArgs>(call.arguments) {
            Ok(args) => args,
            Err(err) => return error(call.id, call.name, err.to_string()),
        };

        let verdict = match args.verdict {
            SubmitReviewVerdictArg::Approve => ReviewVerdict::Approve,
            SubmitReviewVerdictArg::Reject => ReviewVerdict::Reject,
        };
        let category = match args.category {
            Some(category) => Some(category.into_review_category()),
            None => None,
        };
        if matches!(verdict, ReviewVerdict::Approve) && category.is_some() {
            return error(
                call.id,
                call.name,
                "category must be omitted when verdict is approve",
            );
        }
        let findings = args.findings.and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        });
        if matches!(verdict, ReviewVerdict::Reject) {
            if findings.is_none() {
                return error(
                    call.id,
                    call.name,
                    "findings is required when verdict is reject",
                );
            }
            if category.is_none() {
                return error(
                    call.id,
                    call.name,
                    "category is required when verdict is reject",
                );
            }
        }

        let reviewed_hash = match create_checkpoint(&ctx.config.workspace_root) {
            Ok(checkpoint_id) => checkpoint_id,
            Err(err) => return error(call.id, call.name, err.to_string()),
        };
        let checkpoint_id = reviewed_hash.clone();
        let reject_category_event = category.map(ReviewRejectCategory::as_event_category);
        match self
            .store
            .resolve_ticket_with_category(
                &args.ticket_id,
                verdict,
                reviewed_hash.clone(),
                findings.clone(),
                category,
            )
            .await
        {
            Ok(ticket) => {
                let value = json!({
                    "ticket_id": ticket.ticket_id,
                    "goal_id": ticket.goal_id,
                    "verdict": match verdict {
                        ReviewVerdict::Approve => "approve",
                        ReviewVerdict::Reject => "reject",
                    },
                    "reviewed_hash": reviewed_hash,
                    "category": reject_category_event.map(|category| match category {
                        crate::events::ReviewRejectCategoryEvent::RetriableGap => "retriable_gap",
                        crate::events::ReviewRejectCategoryEvent::NeedsUser => "needs_user",
                        crate::events::ReviewRejectCategoryEvent::ExternalBlocker => "external_blocker",
                    }),
                    "findings": findings,
                    "checkpoint_id": checkpoint_id,
                });
                ToolOutcome::from_result(ToolResult {
                    tool_call_id: call.id,
                    tool_name: call.name,
                    status: ToolStatus::Success,
                    content: value.to_string(),
                    meta: Some(value),
                    parts: Vec::new(),
                })
                .with_effect(ToolRuntimeEffect::ReviewSubmitted {
                    ticket_id: ticket.ticket_id,
                    goal_id: ticket.goal_id,
                    verdict: verdict.as_event_verdict(),
                    reviewed_hash,
                    reject_category: reject_category_event,
                    findings,
                    checkpoint_id,
                })
            }
            Err(err) => error(call.id, call.name, err.to_string()),
        }
    }
}

impl SubmitReviewRejectCategoryArg {
    fn into_review_category(self) -> ReviewRejectCategory {
        match self {
            Self::RetriableGap => ReviewRejectCategory::RetriableGap,
            Self::NeedsUser => ReviewRejectCategory::NeedsUser,
            Self::ExternalBlocker => ReviewRejectCategory::ExternalBlocker,
        }
    }
}

fn error(
    tool_call_id: impl Into<String>,
    tool_name: impl Into<String>,
    content: impl Into<String>,
) -> ToolOutcome {
    ToolOutcome::from_result(ToolResult {
        tool_call_id: tool_call_id.into(),
        tool_name: tool_name.into(),
        status: ToolStatus::Error,
        content: content.into(),
        meta: None,
        parts: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::config::AgentConfig;
    use crate::exec_session::ExecSessionManager;
    use crate::index_db::IndexDb;
    use crate::policy::{PolicyManager, PolicyMode};
    use crate::registry::ToolContext;
    use crate::runtime::agent_profile::{profile_for_type, AgentType};
    use crate::runtime::forge::review::{ReviewStore, ReviewVerdict};
    use crate::tools::{ToolHandler, ToolInvocation, ToolRuntimeEffect};
    use crate::types::{ThreadId, ToolCall, ToolStatus};

    async fn fixture() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        ReviewStore,
        ToolContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let db_dir = dir.path().join("db");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("artifact.txt"), "ready for review").unwrap();
        std::process::Command::new("git")
            .arg("init")
            .current_dir(&workspace)
            .output()
            .unwrap();

        let db = IndexDb::open(db_dir.join("index.sqlite")).await.unwrap();
        let store = ReviewStore::new(db);
        let ctx = ToolContext {
            config: AgentConfig {
                workspace_root: workspace.clone(),
                cwd: workspace.clone(),
                policy_mode: PolicyMode::Off,
                ..AgentConfig::default()
            },
            thread_id: Some(ThreadId::new("thread_review_tool")),
            turn_id: None,
            tool_invocation_id: None,
            exec_sessions: Arc::new(ExecSessionManager::default()),
            exec_output_sink: None,
            policy: Arc::new(PolicyManager::default()),
            agent_tool_policy: profile_for_type(Some(AgentType::Reviewer)).tool_policy,
            inbox: None,
            goal_api: None,
        };
        (dir, workspace, store, ctx)
    }

    #[tokio::test]
    async fn submit_review_approve_records_fresh_ticket_and_event() {
        let (_dir, _workspace, store, ctx) = fixture().await;
        let tool = SubmitReviewTool::new(store.clone());
        let ticket = store
            .mint_ticket("goal_1", Some("baseline".to_string()))
            .await
            .unwrap();

        let outcome = tool
            .handle(
                ToolInvocation {
                    invocation_id: "inv_review".to_string(),
                    call: ToolCall {
                        id: "call_review".to_string(),
                        name: "submit_review".to_string(),
                        arguments: serde_json::json!({
                            "ticket_id": ticket.ticket_id,
                            "verdict": "approve"
                        }),
                        thought_signature: None,
                    },
                },
                &ctx,
            )
            .await;

        assert_eq!(outcome.model_result.status, ToolStatus::Success);
        let reviewed_hash = outcome
            .model_result
            .meta
            .as_ref()
            .and_then(|meta| meta["reviewed_hash"].as_str())
            .expect("reviewed hash");
        assert!(store
            .latest_fresh_approval("goal_1", Some(reviewed_hash))
            .await
            .unwrap()
            .is_some());
        assert!(outcome.effects.iter().any(|effect| {
            matches!(
                effect,
                ToolRuntimeEffect::ReviewSubmitted {
                    goal_id,
                    verdict,
                    reviewed_hash: Some(event_hash),
                    ..
                } if goal_id == "goal_1"
                    && verdict == &crate::events::ReviewVerdictEvent::Approve
                    && event_hash == reviewed_hash
            )
        }));
    }

    #[tokio::test]
    async fn submit_review_reject_requires_findings_and_category() {
        let (_dir, _workspace, store, ctx) = fixture().await;
        let tool = SubmitReviewTool::new(store.clone());
        let ticket = store
            .mint_ticket("goal_1", Some("baseline".to_string()))
            .await
            .unwrap();

        let outcome = tool
            .handle(
                ToolInvocation {
                    invocation_id: "inv_review".to_string(),
                    call: ToolCall {
                        id: "call_review".to_string(),
                        name: "submit_review".to_string(),
                        arguments: serde_json::json!({
                            "ticket_id": ticket.ticket_id,
                            "verdict": "reject",
                            "findings": "missing verification"
                        }),
                        thought_signature: None,
                    },
                },
                &ctx,
            )
            .await;

        assert_eq!(outcome.model_result.status, ToolStatus::Error);
        assert!(outcome
            .model_result
            .content
            .contains("category is required"));
    }

    #[tokio::test]
    async fn submit_review_reject_records_category() {
        let (_dir, _workspace, store, ctx) = fixture().await;
        let tool = SubmitReviewTool::new(store.clone());
        let ticket = store
            .mint_ticket("goal_1", Some("baseline".to_string()))
            .await
            .unwrap();

        let outcome = tool
            .handle(
                ToolInvocation {
                    invocation_id: "inv_review".to_string(),
                    call: ToolCall {
                        id: "call_review".to_string(),
                        name: "submit_review".to_string(),
                        arguments: serde_json::json!({
                            "ticket_id": ticket.ticket_id,
                            "verdict": "reject",
                            "category": "retriable_gap",
                            "findings": "missing verification"
                        }),
                        thought_signature: None,
                    },
                },
                &ctx,
            )
            .await;

        assert_eq!(outcome.model_result.status, ToolStatus::Success);
        let reviewed = store
            .latest_ticket("goal_1")
            .await
            .unwrap()
            .expect("resolved ticket");
        assert_eq!(reviewed.verdict(), Some(ReviewVerdict::Reject));
        assert_eq!(
            reviewed
                .reject_category
                .map(crate::runtime::forge::review::ReviewRejectCategory::as_str),
            Some("retriable_gap")
        );
    }
}
