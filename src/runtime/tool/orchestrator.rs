use std::sync::Arc;

use anyhow::Result;

use crate::events::RuntimeEventKind;
use crate::registry::ToolContext;
use crate::runtime::thread_session::{ActiveToolInvocation, LiveEventSink, ThreadEventRecorder};
use crate::runtime::tool::effects::ToolExecutionOutcome;
use crate::runtime::tool::hooks::{ToolHooks, ToolInvocationContext};
use crate::runtime::tool::resolver::ToolResolver;
use crate::runtime::tool::selection::authorize_tool;
use crate::session::ThreadSnapshot;
use crate::tools::{ToolCapabilities, ToolInvocation, ToolOutcome, ToolRuntimeEffect};
use crate::types::{ToolCall, ToolResult, ToolStatus, TurnId};

#[derive(Clone)]
pub(crate) struct ToolOrchestrator {
    resolver: ToolResolver,
    hooks: Arc<dyn ToolHooks>,
}

impl ToolOrchestrator {
    #[cfg(test)]
    pub(crate) fn new(resolver: ToolResolver) -> Self {
        Self {
            resolver,
            hooks: Arc::new(crate::runtime::tool::hooks::NoopToolHooks),
        }
    }

    pub(crate) fn with_hooks(resolver: ToolResolver, hooks: Arc<dyn ToolHooks>) -> Self {
        Self { resolver, hooks }
    }

    #[cfg(test)]
    pub(crate) async fn invoke(&self, call: ToolCall, ctx: &ToolContext) -> ToolOutcome {
        if !authorize_tool(&call.name, &ctx.agent_tool_policy) {
            return denied_by_agent_profile_outcome(call);
        }
        let invocation = ToolInvocation {
            invocation_id: format!("inv_{}", call.id),
            call,
        };

        match self.resolver.resolve(&invocation.call) {
            Some(handler) => handler.handle(invocation, ctx).await,
            None => unknown_tool_outcome(invocation.call),
        }
    }

    pub(crate) async fn execute_with_lifecycle(
        &self,
        call: ToolCall,
        ctx: &ToolContext,
        recorder: &mut ThreadEventRecorder,
        snapshot: &ThreadSnapshot,
        turn_id: &TurnId,
    ) -> Result<ToolExecutionOutcome> {
        let invocation_id = format!("inv_{}", call.id);
        let handler = self.resolver.resolve(&call);
        let capabilities = handler
            .as_ref()
            .map(|handler| handler.capabilities())
            .unwrap_or_else(ToolCapabilities::read_only);
        let hook_ctx = ToolInvocationContext {
            invocation_id: invocation_id.clone(),
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            arguments: call.arguments.clone(),
            thread_id: ctx.thread_id.clone(),
            workspace_root: ctx.config.workspace_root.clone(),
            capabilities: capabilities.clone(),
        };

        recorder.record(
            snapshot,
            Some(turn_id),
            RuntimeEventKind::ToolInvocationStarted {
                invocation_id: invocation_id.clone(),
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                mutating: hook_ctx.capabilities.mutating,
            },
        )?;
        recorder.mark_tool_invocation_active(ActiveToolInvocation {
            invocation_id: hook_ctx.invocation_id.clone(),
            tool_call_id: hook_ctx.tool_call_id.clone(),
            tool_name: hook_ctx.tool_name.clone(),
        })?;

        if !authorize_tool(&call.name, &ctx.agent_tool_policy) {
            let outcome = denied_by_agent_profile_outcome(call);
            recorder.clear_tool_invocation(&hook_ctx.invocation_id)?;
            recorder.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::ToolInvocationFailed {
                    invocation_id: hook_ctx.invocation_id.clone(),
                    tool_call_id: hook_ctx.tool_call_id.clone(),
                    tool_name: hook_ctx.tool_name.clone(),
                    message: outcome.model_result.content.clone(),
                },
            )?;
            return Ok(ToolExecutionOutcome::from_tool_outcome(outcome));
        }

        let mut hook_effects = self.hooks.before_invocation(&hook_ctx).await?;
        if hook_effects
            .iter()
            .any(|effect| matches!(effect, ToolRuntimeEffect::ApprovalRequested { .. }))
        {
            let mut outcome = approval_required_outcome(&hook_ctx, hook_effects);
            self.record_approval_requested_hooks_and_events(
                &hook_ctx,
                &mut outcome,
                recorder,
                snapshot,
                turn_id,
            )
            .await?;
            recorder.clear_tool_invocation(&hook_ctx.invocation_id)?;
            return Ok(ToolExecutionOutcome::from_tool_outcome(outcome));
        }
        hook_effects.extend(self.hooks.before_handler_execution(&hook_ctx).await?);
        if hook_effects
            .iter()
            .any(|effect| matches!(effect, ToolRuntimeEffect::ApprovalRequested { .. }))
        {
            let mut outcome = approval_required_outcome(&hook_ctx, hook_effects);
            self.record_approval_requested_hooks_and_events(
                &hook_ctx,
                &mut outcome,
                recorder,
                snapshot,
                turn_id,
            )
            .await?;
            recorder.clear_tool_invocation(&hook_ctx.invocation_id)?;
            return Ok(ToolExecutionOutcome::from_tool_outcome(outcome));
        }
        if let Some(result) = short_circuit_result(&hook_effects) {
            record_short_circuit_result(&hook_ctx, &result, recorder, snapshot, turn_id)?;
            return Ok(ToolExecutionOutcome::from_tool_outcome(
                ToolOutcome::from_result(result),
            ));
        }

        let invocation = ToolInvocation {
            invocation_id,
            call,
        };
        let mut handler_ctx = ctx.clone();
        handler_ctx.tool_invocation_id = Some(hook_ctx.invocation_id.clone());
        let mut outcome = match handler {
            Some(handler) => handler.handle(invocation, &handler_ctx).await,
            None => unknown_tool_outcome(invocation.call),
        };
        outcome.effects.splice(0..0, hook_effects);

        self.record_approval_requested_hooks_and_events(
            &hook_ctx,
            &mut outcome,
            recorder,
            snapshot,
            turn_id,
        )
        .await?;
        self.record_user_input_requested_events(&hook_ctx, &outcome, recorder, snapshot, turn_id)?;

        let after_completion_effects = self
            .hooks
            .after_handler_completion(&hook_ctx, &outcome)
            .await?;
        outcome.effects.extend(after_completion_effects);

        if matches!(outcome.model_result.status, ToolStatus::Error) {
            outcome.effects.extend(
                self.hooks
                    .failed(&hook_ctx, &outcome.model_result.content)
                    .await?,
            );
            recorder.clear_tool_invocation(&hook_ctx.invocation_id)?;
            recorder.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::ToolInvocationFailed {
                    invocation_id: hook_ctx.invocation_id.clone(),
                    tool_call_id: hook_ctx.tool_call_id.clone(),
                    tool_name: hook_ctx.tool_name.clone(),
                    message: outcome.model_result.content.clone(),
                },
            )?;
        } else if !matches!(outcome.model_result.status, ToolStatus::ReviewRequired) {
            recorder.clear_tool_invocation(&hook_ctx.invocation_id)?;
            recorder.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::ToolInvocationCompleted {
                    invocation_id: hook_ctx.invocation_id.clone(),
                    tool_call_id: hook_ctx.tool_call_id.clone(),
                    tool_name: hook_ctx.tool_name.clone(),
                    status: outcome.model_result.status.clone(),
                },
            )?;
        } else {
            recorder.clear_tool_invocation(&hook_ctx.invocation_id)?;
        }

        Ok(ToolExecutionOutcome::from_tool_outcome(outcome))
    }

    async fn record_approval_requested_hooks_and_events(
        &self,
        hook_ctx: &ToolInvocationContext,
        outcome: &mut ToolOutcome,
        recorder: &mut ThreadEventRecorder,
        snapshot: &ThreadSnapshot,
        turn_id: &TurnId,
    ) -> Result<()> {
        for effect in outcome.effects.clone() {
            if let ToolRuntimeEffect::ApprovalRequested {
                approval_id,
                reason,
                ..
            } = effect
            {
                outcome.effects.extend(
                    self.hooks
                        .approval_requested(hook_ctx, &approval_id)
                        .await?,
                );
                recorder.record(
                    snapshot,
                    Some(turn_id),
                    RuntimeEventKind::ToolInvocationWaitingApproval {
                        invocation_id: hook_ctx.invocation_id.clone(),
                        approval_id,
                        reason,
                    },
                )?;
            }
        }
        Ok(())
    }

    fn record_user_input_requested_events(
        &self,
        hook_ctx: &ToolInvocationContext,
        outcome: &ToolOutcome,
        recorder: &mut ThreadEventRecorder,
        snapshot: &ThreadSnapshot,
        turn_id: &TurnId,
    ) -> Result<()> {
        for effect in &outcome.effects {
            if let ToolRuntimeEffect::UserInputRequested { request_id, .. } = effect {
                recorder.record(
                    snapshot,
                    Some(turn_id),
                    RuntimeEventKind::ToolInvocationWaitingUserInput {
                        invocation_id: hook_ctx.invocation_id.clone(),
                        request_id: request_id.clone(),
                        reason: "waiting for user input".to_string(),
                    },
                )?;
            }
        }
        Ok(())
    }
}

fn unknown_tool_outcome(call: ToolCall) -> ToolOutcome {
    ToolOutcome::from_result(ToolResult {
        tool_call_id: call.id,
        tool_name: call.name.clone(),
        status: ToolStatus::Error,
        content: format!("Unknown tool: {}", call.name),
        meta: None,
        parts: Vec::new(),
    })
}

fn denied_by_agent_profile_outcome(call: ToolCall) -> ToolOutcome {
    ToolOutcome::from_result(ToolResult {
        tool_call_id: call.id,
        tool_name: call.name.clone(),
        status: ToolStatus::Error,
        content: format!("Tool denied by agent profile: {}", call.name),
        meta: None,
        parts: Vec::new(),
    })
}

fn approval_required_outcome(
    hook_ctx: &ToolInvocationContext,
    effects: Vec<ToolRuntimeEffect>,
) -> ToolOutcome {
    let reason = effects
        .iter()
        .find_map(|effect| match effect {
            ToolRuntimeEffect::ApprovalRequested { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "approval required".to_string());
    ToolOutcome::from_result(ToolResult {
        tool_call_id: hook_ctx.tool_call_id.clone(),
        tool_name: hook_ctx.tool_name.clone(),
        status: ToolStatus::ReviewRequired,
        content: reason,
        meta: None,
        parts: Vec::new(),
    })
    .with_effects(effects)
}

fn short_circuit_result(effects: &[ToolRuntimeEffect]) -> Option<ToolResult> {
    effects.iter().find_map(|effect| match effect {
        ToolRuntimeEffect::ShortCircuit { result } => Some(result.clone()),
        _ => None,
    })
}

fn record_short_circuit_result(
    hook_ctx: &ToolInvocationContext,
    result: &ToolResult,
    recorder: &mut ThreadEventRecorder,
    snapshot: &ThreadSnapshot,
    turn_id: &TurnId,
) -> Result<()> {
    recorder.clear_tool_invocation(&hook_ctx.invocation_id)?;
    if matches!(result.status, ToolStatus::Error) {
        recorder.record(
            snapshot,
            Some(turn_id),
            RuntimeEventKind::ToolInvocationFailed {
                invocation_id: hook_ctx.invocation_id.clone(),
                tool_call_id: hook_ctx.tool_call_id.clone(),
                tool_name: hook_ctx.tool_name.clone(),
                message: result.content.clone(),
            },
        )?;
    } else if !matches!(result.status, ToolStatus::ReviewRequired) {
        recorder.record(
            snapshot,
            Some(turn_id),
            RuntimeEventKind::ToolInvocationCompleted {
                invocation_id: hook_ctx.invocation_id.clone(),
                tool_call_id: hook_ctx.tool_call_id.clone(),
                tool_name: hook_ctx.tool_name.clone(),
                status: result.status.clone(),
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::config::AgentConfig;
    use crate::events::RuntimeEvent;
    use crate::exec_session::ExecSessionManager;
    use crate::policy::{PolicyManager, PolicyMode};
    use crate::registry::ToolRegistry;
    use crate::runtime::agent_profile::{profile_for_type, AgentToolPolicy, AgentType};
    use crate::runtime::thread_runtime::ThreadRuntimeStatus;
    use crate::runtime::thread_session::ThreadSessionLiveState;
    use crate::session::ApprovalId;
    use crate::tools::read_file::ReadFileTool;
    use crate::tools::run_command::RunCommandTool;
    use crate::tools::{ToolRuntimeEffect, ToolSpec};
    use crate::types::{ThreadId, TurnId};
    use async_trait::async_trait;
    use tokio::sync::broadcast;

    fn tool_context(policy_mode: PolicyMode) -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext {
            config: AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                policy_mode,
                ..AgentConfig::default()
            },
            thread_id: Some(ThreadId::new("thread_orchestrator")),
            turn_id: Some(TurnId::new("turn_orchestrator")),
            tool_invocation_id: None,
            exec_sessions: Arc::new(ExecSessionManager::default()),
            exec_output_sink: None,
            policy: Arc::new(PolicyManager::default()),
            agent_tool_policy: AgentToolPolicy::all(),
            inbox: None,
            goal_api: None,
            memory_api: None,
        };
        (dir, ctx)
    }

    #[derive(Default)]
    struct CountingHooks {
        before_handler_calls: AtomicUsize,
    }

    struct GateApprovalHooks;

    #[derive(Default)]
    struct ArgumentCaptureHooks {
        before_invocation_status: Mutex<Option<String>>,
        before_handler_status: Mutex<Option<String>>,
    }

    struct ShortCircuitHooks;

    struct RecordingTool {
        called: Arc<AtomicBool>,
    }

    struct SubmitReviewRecordingTool {
        called: Arc<AtomicBool>,
    }

    #[async_trait]
    impl ToolHooks for CountingHooks {
        async fn before_invocation(
            &self,
            _ctx: &ToolInvocationContext,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            Ok(Vec::new())
        }

        async fn approval_requested(
            &self,
            _ctx: &ToolInvocationContext,
            _approval_id: &ApprovalId,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            Ok(Vec::new())
        }

        async fn before_handler_execution(
            &self,
            _ctx: &ToolInvocationContext,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            self.before_handler_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }

        async fn after_handler_completion(
            &self,
            _ctx: &ToolInvocationContext,
            _outcome: &ToolOutcome,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            Ok(Vec::new())
        }

        async fn failed(
            &self,
            _ctx: &ToolInvocationContext,
            _message: &str,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl ToolHooks for GateApprovalHooks {
        async fn before_invocation(
            &self,
            ctx: &ToolInvocationContext,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            Ok(vec![ToolRuntimeEffect::ApprovalRequested {
                approval_id: ApprovalId::new("approval_hook_gate"),
                tool_name: ctx.tool_name.clone(),
                reason: "hook approval required".to_string(),
                checkpoint_id: None,
                permission_profile: crate::config::PermissionProfile::FullAccess,
                filesystem_sandbox: "none".to_string(),
                network_sandbox: "none".to_string(),
                env_isolation: "none".to_string(),
                command: None,
            }])
        }

        async fn approval_requested(
            &self,
            _ctx: &ToolInvocationContext,
            _approval_id: &ApprovalId,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            Ok(Vec::new())
        }

        async fn before_handler_execution(
            &self,
            _ctx: &ToolInvocationContext,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            panic!("before_handler_execution should not run after pre-invocation approval gate")
        }

        async fn after_handler_completion(
            &self,
            _ctx: &ToolInvocationContext,
            _outcome: &ToolOutcome,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            Ok(Vec::new())
        }

        async fn failed(
            &self,
            _ctx: &ToolInvocationContext,
            _message: &str,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl ToolHooks for ArgumentCaptureHooks {
        async fn before_invocation(
            &self,
            ctx: &ToolInvocationContext,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            *self.before_invocation_status.lock().unwrap() = ctx
                .arguments
                .get("status")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            Ok(Vec::new())
        }

        async fn approval_requested(
            &self,
            _ctx: &ToolInvocationContext,
            _approval_id: &ApprovalId,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            Ok(Vec::new())
        }

        async fn before_handler_execution(
            &self,
            ctx: &ToolInvocationContext,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            *self.before_handler_status.lock().unwrap() = ctx
                .arguments
                .get("status")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            Ok(Vec::new())
        }

        async fn after_handler_completion(
            &self,
            _ctx: &ToolInvocationContext,
            _outcome: &ToolOutcome,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            Ok(Vec::new())
        }

        async fn failed(
            &self,
            _ctx: &ToolInvocationContext,
            _message: &str,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl ToolHooks for ShortCircuitHooks {
        async fn before_invocation(
            &self,
            _ctx: &ToolInvocationContext,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            Ok(Vec::new())
        }

        async fn approval_requested(
            &self,
            _ctx: &ToolInvocationContext,
            _approval_id: &ApprovalId,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            Ok(Vec::new())
        }

        async fn before_handler_execution(
            &self,
            ctx: &ToolInvocationContext,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            Ok(vec![ToolRuntimeEffect::ShortCircuit {
                result: ToolResult {
                    tool_call_id: ctx.tool_call_id.clone(),
                    tool_name: ctx.tool_name.clone(),
                    status: ToolStatus::Error,
                    content: "short-circuited by hook".to_string(),
                    meta: None,
                    parts: Vec::new(),
                },
            }])
        }

        async fn after_handler_completion(
            &self,
            _ctx: &ToolInvocationContext,
            _outcome: &ToolOutcome,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            Ok(Vec::new())
        }

        async fn failed(
            &self,
            _ctx: &ToolInvocationContext,
            _message: &str,
        ) -> Result<Vec<ToolRuntimeEffect>> {
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl crate::tools::ToolHandler for RecordingTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec::function(
                "recording_tool",
                "Record whether the handler ran",
                serde_json::json!({"type": "object", "additionalProperties": false}),
            )
        }

        fn capabilities(&self) -> ToolCapabilities {
            ToolCapabilities::mutating(true)
        }

        async fn handle(&self, invocation: ToolInvocation, _ctx: &ToolContext) -> ToolOutcome {
            self.called.store(true, Ordering::SeqCst);
            ToolOutcome::success(
                invocation.call.id,
                invocation.call.name,
                crate::tools::ToolModelOutput::text("handler ran"),
            )
        }
    }

    #[async_trait]
    impl crate::tools::ToolHandler for SubmitReviewRecordingTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec::function(
                "submit_review",
                "Record whether submit_review ran",
                serde_json::json!({"type": "object", "additionalProperties": false}),
            )
        }

        fn capabilities(&self) -> ToolCapabilities {
            ToolCapabilities::read_only()
        }

        async fn handle(&self, invocation: ToolInvocation, _ctx: &ToolContext) -> ToolOutcome {
            self.called.store(true, Ordering::SeqCst);
            ToolOutcome::success(
                invocation.call.id,
                invocation.call.name,
                crate::tools::ToolModelOutput::text("review submitted"),
            )
        }
    }

    fn recorder_for(
        thread_id: ThreadId,
        snapshot: ThreadSnapshot,
        path: std::path::PathBuf,
    ) -> ThreadEventRecorder {
        let (event_tx, _) = broadcast::channel(16);
        let live_state = Arc::new(std::sync::RwLock::new(ThreadSessionLiveState {
            snapshot,
            overlay: crate::runtime::thread_session::RuntimeOverlay::default(),
            events: vec![],
            status: ThreadRuntimeStatus::Idle,
        }));
        ThreadEventRecorder::new(
            thread_id,
            crate::state::rollout::RolloutStore::new(path),
            event_tx,
            live_state,
            1,
            64,
        )
    }

    #[tokio::test]
    async fn orchestrator_invokes_registered_handler_and_returns_typed_effects() {
        let (_dir, ctx) = tool_context(PolicyMode::Enforced);
        let mut registry = ToolRegistry::new();
        registry.register(RunCommandTool);
        let orchestrator = ToolOrchestrator::new(ToolResolver::new(registry));

        let outcome = orchestrator
            .invoke(
                ToolCall {
                    id: "call_approval".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({ "command": "rm -rf scratch" }),
                    thought_signature: None,
                },
                &ctx,
            )
            .await;

        assert!(outcome.effects.iter().any(|effect| {
            matches!(effect, ToolRuntimeEffect::ApprovalRequested { tool_name, .. }
                if tool_name == "run_command")
        }));
    }

    #[tokio::test]
    async fn orchestrator_returns_error_for_unknown_tool() {
        let (_dir, ctx) = tool_context(PolicyMode::Off);
        let orchestrator = ToolOrchestrator::new(ToolResolver::new(ToolRegistry::new()));

        let outcome = orchestrator
            .invoke(
                ToolCall {
                    id: "call_unknown".into(),
                    name: "missing_tool".into(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                },
                &ctx,
            )
            .await;

        assert_eq!(outcome.model_result.status.as_str(), "error");
        assert!(outcome.model_result.content.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn orchestrator_denies_tool_blocked_by_agent_policy() {
        let (_dir, mut ctx) = tool_context(PolicyMode::Off);
        ctx.agent_tool_policy = AgentToolPolicy::read_only_basic_collaboration();
        let called = Arc::new(AtomicBool::new(false));
        let mut registry = ToolRegistry::new();
        registry.register(RecordingTool {
            called: called.clone(),
        });
        let orchestrator = ToolOrchestrator::new(ToolResolver::new(registry));

        let outcome = orchestrator
            .invoke(
                ToolCall {
                    id: "call_blocked".into(),
                    name: "recording_tool".into(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                },
                &ctx,
            )
            .await;

        assert_eq!(outcome.model_result.status, ToolStatus::Error);
        assert!(outcome
            .model_result
            .content
            .contains("denied by agent profile"));
        assert!(!called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn orchestrator_denies_worker_submit_review_even_if_call_is_fabricated() {
        let (_dir, mut ctx) = tool_context(PolicyMode::Off);
        ctx.agent_tool_policy = profile_for_type(Some(AgentType::Worker)).tool_policy;
        let called = Arc::new(AtomicBool::new(false));
        let mut registry = ToolRegistry::new();
        registry.register(SubmitReviewRecordingTool {
            called: called.clone(),
        });
        let orchestrator = ToolOrchestrator::new(ToolResolver::new(registry));

        let outcome = orchestrator
            .invoke(
                ToolCall {
                    id: "call_submit_review_worker".into(),
                    name: "submit_review".into(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                },
                &ctx,
            )
            .await;

        assert_eq!(outcome.model_result.status, ToolStatus::Error);
        assert!(outcome
            .model_result
            .content
            .contains("denied by agent profile"));
        assert!(!called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn orchestrator_allows_reviewer_submit_review_direct_execution() {
        let (_dir, mut ctx) = tool_context(PolicyMode::Off);
        ctx.agent_tool_policy = profile_for_type(Some(AgentType::Reviewer)).tool_policy;
        let called = Arc::new(AtomicBool::new(false));
        let mut registry = ToolRegistry::new();
        registry.register(SubmitReviewRecordingTool {
            called: called.clone(),
        });
        let orchestrator = ToolOrchestrator::new(ToolResolver::new(registry));

        let outcome = orchestrator
            .invoke(
                ToolCall {
                    id: "call_submit_review_reviewer".into(),
                    name: "submit_review".into(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                },
                &ctx,
            )
            .await;

        assert_eq!(outcome.model_result.status, ToolStatus::Success);
        assert!(called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn orchestrator_uses_injected_hooks_for_lifecycle_execution() {
        let (dir, ctx) = tool_context(PolicyMode::Off);
        std::fs::write(dir.path().join("notes.txt"), "hello").unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(ReadFileTool);
        let hooks = Arc::new(CountingHooks::default());
        let orchestrator = ToolOrchestrator::with_hooks(ToolResolver::new(registry), hooks.clone());
        let thread_id = ThreadId::new("thread_hooked_orchestrator");
        let snapshot = ThreadSnapshot::new_thread(
            thread_id.clone(),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let rollout_path = dir.path().join("hooked-rollout.jsonl");
        let mut recorder = recorder_for(thread_id, snapshot.clone(), rollout_path);

        let outcome = orchestrator
            .execute_with_lifecycle(
                ToolCall {
                    id: "call_hooked_read".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({ "path": "notes.txt" }),
                    thought_signature: None,
                },
                &ctx,
                &mut recorder,
                &snapshot,
                &TurnId::new("turn_hooked_orchestrator"),
            )
            .await
            .expect("execute lifecycle");

        assert_eq!(outcome.result.status.as_str(), "success");
        assert_eq!(hooks.before_handler_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn lifecycle_hooks_receive_tool_arguments_before_handler_runs() {
        let (dir, ctx) = tool_context(PolicyMode::Off);
        let called = Arc::new(AtomicBool::new(false));
        let mut registry = ToolRegistry::new();
        registry.register(RecordingTool {
            called: called.clone(),
        });
        let hooks = Arc::new(ArgumentCaptureHooks::default());
        let orchestrator = ToolOrchestrator::with_hooks(ToolResolver::new(registry), hooks.clone());
        let thread_id = ThreadId::new("thread_hook_arguments");
        let snapshot = ThreadSnapshot::new_thread(
            thread_id.clone(),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let rollout_path = dir.path().join("hook-arguments-rollout.jsonl");
        let mut recorder = recorder_for(thread_id, snapshot.clone(), rollout_path);

        let outcome = orchestrator
            .execute_with_lifecycle(
                ToolCall {
                    id: "call_hook_arguments".into(),
                    name: "recording_tool".into(),
                    arguments: serde_json::json!({ "status": "complete" }),
                    thought_signature: None,
                },
                &ctx,
                &mut recorder,
                &snapshot,
                &TurnId::new("turn_hook_arguments"),
            )
            .await
            .expect("execute lifecycle");

        assert_eq!(outcome.result.status, ToolStatus::Success);
        assert!(called.load(Ordering::SeqCst));
        assert_eq!(
            hooks.before_invocation_status.lock().unwrap().as_deref(),
            Some("complete")
        );
        assert_eq!(
            hooks.before_handler_status.lock().unwrap().as_deref(),
            Some("complete")
        );
    }

    #[tokio::test]
    async fn before_handler_short_circuit_skips_handler_and_returns_substitute_result() {
        let (dir, ctx) = tool_context(PolicyMode::Off);
        let called = Arc::new(AtomicBool::new(false));
        let mut registry = ToolRegistry::new();
        registry.register(RecordingTool {
            called: called.clone(),
        });
        let orchestrator =
            ToolOrchestrator::with_hooks(ToolResolver::new(registry), Arc::new(ShortCircuitHooks));
        let thread_id = ThreadId::new("thread_hook_short_circuit");
        let snapshot = ThreadSnapshot::new_thread(
            thread_id.clone(),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let rollout_path = dir.path().join("hook-short-circuit-rollout.jsonl");
        let mut recorder = recorder_for(thread_id, snapshot.clone(), rollout_path);

        let outcome = orchestrator
            .execute_with_lifecycle(
                ToolCall {
                    id: "call_hook_short_circuit".into(),
                    name: "recording_tool".into(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                },
                &ctx,
                &mut recorder,
                &snapshot,
                &TurnId::new("turn_hook_short_circuit"),
            )
            .await
            .expect("execute lifecycle");

        assert_eq!(outcome.result.status, ToolStatus::Error);
        assert_eq!(outcome.result.content, "short-circuited by hook");
        assert!(!called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn orchestrator_pre_invocation_approval_gate_skips_handler() {
        let (dir, ctx) = tool_context(PolicyMode::Off);
        let called = Arc::new(AtomicBool::new(false));
        let mut registry = ToolRegistry::new();
        registry.register(RecordingTool {
            called: called.clone(),
        });
        let orchestrator =
            ToolOrchestrator::with_hooks(ToolResolver::new(registry), Arc::new(GateApprovalHooks));
        let thread_id = ThreadId::new("thread_hook_gate");
        let snapshot = ThreadSnapshot::new_thread(
            thread_id.clone(),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let rollout_path = dir.path().join("hook-gate-rollout.jsonl");
        let mut recorder = recorder_for(thread_id, snapshot.clone(), rollout_path.clone());

        let outcome = orchestrator
            .execute_with_lifecycle(
                ToolCall {
                    id: "call_hook_gate".into(),
                    name: "recording_tool".into(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                },
                &ctx,
                &mut recorder,
                &snapshot,
                &TurnId::new("turn_hook_gate"),
            )
            .await
            .expect("execute lifecycle");

        assert_eq!(outcome.result.status.as_str(), "review_required");
        assert!(!called.load(Ordering::SeqCst));
        let rollout_items = crate::state::rollout::RolloutStore::read_items_blocking(&rollout_path)
            .expect("read rollout items");
        assert!(rollout_items.iter().any(|item| matches!(
            item,
            crate::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                kind: RuntimeEventKind::ToolInvocationWaitingApproval {
                    approval_id,
                    ..
                },
                ..
            }) if approval_id.as_str() == "approval_hook_gate"
        )));
    }
}
