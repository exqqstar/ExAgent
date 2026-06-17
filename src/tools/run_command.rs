use std::path::PathBuf;
use std::process::Stdio;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::time::{timeout, Duration};

use crate::events::ApprovalCommandPayload;
use crate::policy::PolicyDecision;
use crate::registry::ToolContext;
use crate::runtime::process_cleanup::{
    cleanup_child_process_tree, configure_process_group, ProcessCleanupReason,
};
use crate::session::{ApprovalId, ApprovalStatus};
use crate::session::{ExecSessionId, ExecSessionStatus};
use crate::tools::output_projection::{output_projection_meta, project_output};
use crate::tools::{
    ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolRuntimeEffect, ToolSpec,
};
use crate::types::{ToolCall, ToolResult, ToolStatus};
use crate::workspace::resolve_workspace_path;
use crate::workspace_checkpoint::create_checkpoint;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunCommandArgs {
    /// Shell command line to run. Required for a new command; omit when answering an
    /// approval (`approval_id`) or driving an existing session (`exec_session_id`).
    pub command: Option<String>,
    /// Working directory for the command, relative to the workspace root. Defaults to the workspace root.
    pub cwd: Option<String>,
    /// Maximum seconds to wait before the command is timed out. Defaults to the configured command timeout.
    pub timeout_secs: Option<u64>,
    /// When true, start a long-lived session and return an `exec_session_id` instead of blocking until exit.
    pub persistent: Option<bool>,
    /// Target an existing persistent session (from a previous `persistent` run) instead of starting a new command.
    pub exec_session_id: Option<String>,
    /// Text to write to the session's stdin (used with `exec_session_id`).
    pub stdin: Option<String>,
    /// When true, terminate the targeted `exec_session_id`.
    pub terminate: Option<bool>,
    /// Approval id being answered when a prior run required approval.
    pub approval_id: Option<String>,
    /// Approval decision for `approval_id`: "approved" or "denied".
    pub decision: Option<String>,
}

pub struct RunCommandTool;

#[async_trait]
impl ToolHandler for RunCommandTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "run_command",
            "Run a shell command inside the workspace",
            serde_json::to_value(schemars::schema_for!(RunCommandArgs)).unwrap(),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::mutating(true)
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args: RunCommandArgs = match serde_json::from_value(call.arguments.clone()) {
            Ok(args) => args,
            Err(err) => {
                return ToolOutcome::from_result(ToolResult {
                    tool_call_id: call.id,
                    tool_name: call.name,
                    status: ToolStatus::Error,
                    content: err.to_string(),
                    meta: None,
                    parts: Vec::new(),
                });
            }
        };

        handle_run_command_args(call, args, ctx, "run_command").await
    }
}

pub(crate) struct CommandOutcome {
    status: ToolStatus,
    content: String,
    meta: Value,
    effects: Vec<ToolRuntimeEffect>,
}

pub(crate) async fn handle_run_command_args(
    call: ToolCall,
    args: RunCommandArgs,
    ctx: &ToolContext,
    approval_tool_name: &str,
) -> ToolOutcome {
    match run_command(&args, ctx, approval_tool_name).await {
        Ok(result) => ToolOutcome::from_result(ToolResult {
            tool_call_id: call.id,
            tool_name: call.name,
            status: result.status,
            content: result.content,
            meta: Some(result.meta),
            parts: Vec::new(),
        })
        .with_effects(result.effects),
        Err(err) => ToolOutcome::from_result(ToolResult {
            tool_call_id: call.id,
            tool_name: call.name,
            status: ToolStatus::Error,
            content: err,
            meta: None,
            parts: Vec::new(),
        }),
    }
}

async fn run_command(
    args: &RunCommandArgs,
    ctx: &ToolContext,
    approval_tool_name: &str,
) -> Result<CommandOutcome, String> {
    if let Some(approval_id) = &args.approval_id {
        return handle_approval_decision(args, ctx, ApprovalId::new(approval_id)).await;
    }

    if let Some(exec_session_id) = &args.exec_session_id {
        return run_persistent_command(args, ctx, ExecSessionId::new(exec_session_id)).await;
    }

    if args.persistent.unwrap_or(false) {
        return start_persistent_command(args, ctx, approval_tool_name).await;
    }

    let command_text = args
        .command
        .as_deref()
        .ok_or_else(|| "command is required".to_string())?;
    let cwd = resolve_cwd(args, ctx)?;
    let timeout_secs = args.timeout_secs.unwrap_or(ctx.config.command_timeout_secs);
    if let Some(outcome) = maybe_require_approval(
        ctx,
        approval_tool_name,
        command_text,
        &cwd,
        args.timeout_secs,
        false,
    )
    .await?
    {
        return Ok(outcome);
    }

    run_one_shot_command(command_text, cwd, timeout_secs, ctx).await
}

async fn start_persistent_command(
    args: &RunCommandArgs,
    ctx: &ToolContext,
    approval_tool_name: &str,
) -> Result<CommandOutcome, String> {
    let command = args
        .command
        .as_deref()
        .ok_or_else(|| "command is required".to_string())?;
    let cwd = resolve_cwd(args, ctx)?;
    let thread_id = ctx
        .thread_id
        .as_ref()
        .ok_or_else(|| "persistent exec sessions require a runtime thread_id".to_string())?;
    if let Some(outcome) =
        maybe_require_approval(ctx, approval_tool_name, command, &cwd, None, true).await?
    {
        return Ok(outcome);
    }
    let snapshot = ctx
        .exec_sessions
        .start(
            &ctx.config.workspace_root,
            thread_id,
            ctx.turn_id.clone(),
            ctx.tool_invocation_id.clone(),
            command,
            cwd,
            ctx.exec_output_sink.clone(),
        )
        .await?;

    Ok(persistent_outcome(snapshot, ctx.config.max_output_bytes))
}

async fn handle_approval_decision(
    args: &RunCommandArgs,
    ctx: &ToolContext,
    approval_id: ApprovalId,
) -> Result<CommandOutcome, String> {
    let decision = args
        .decision
        .as_deref()
        .ok_or_else(|| "decision is required when approval_id is provided".to_string())?;
    let pending = ctx.policy.take_pending_command(&approval_id).await?;

    match decision {
        "approved" => {
            if pending.persistent {
                let snapshot = ctx
                    .exec_sessions
                    .start(
                        &ctx.config.workspace_root,
                        &pending.thread_id,
                        ctx.turn_id.clone(),
                        ctx.tool_invocation_id.clone(),
                        &pending.command,
                        pending.cwd.clone(),
                        ctx.exec_output_sink.clone(),
                    )
                    .await?;
                let mut outcome = persistent_outcome(snapshot, ctx.config.max_output_bytes);
                annotate_policy_meta(
                    &mut outcome.meta,
                    &approval_id,
                    ApprovalStatus::Approved,
                    "allow",
                    Some(pending.reason.as_str()),
                );
                annotate_checkpoint_meta(&mut outcome.meta, pending.checkpoint_id.as_deref());
                outcome.effects.push(ToolRuntimeEffect::ApprovalApproved {
                    approval_id,
                    note: None,
                });
                Ok(outcome)
            } else {
                let mut outcome = run_one_shot_command(
                    &pending.command,
                    pending.cwd.clone(),
                    pending
                        .timeout_secs
                        .unwrap_or(ctx.config.command_timeout_secs),
                    ctx,
                )
                .await?;
                annotate_policy_meta(
                    &mut outcome.meta,
                    &approval_id,
                    ApprovalStatus::Approved,
                    "allow",
                    Some(pending.reason.as_str()),
                );
                annotate_checkpoint_meta(&mut outcome.meta, pending.checkpoint_id.as_deref());
                outcome.effects.push(ToolRuntimeEffect::ApprovalApproved {
                    approval_id,
                    note: None,
                });
                Ok(outcome)
            }
        }
        "denied" => {
            let mut meta = json!({
                "approval_id": approval_id.as_str(),
                "approval_status": "denied",
                "policy_decision": "deny",
                "approval_reason": pending.reason,
            });
            annotate_checkpoint_meta(&mut meta, pending.checkpoint_id.as_deref());
            merge_object_meta(&mut meta, permission_profile_meta(ctx));
            Ok(CommandOutcome {
                status: ToolStatus::Error,
                content: "Approval denied".into(),
                meta,
                effects: vec![ToolRuntimeEffect::ApprovalDenied {
                    approval_id,
                    note: None,
                }],
            })
        }
        other => Err(format!("unsupported approval decision: {other}")),
    }
}

async fn run_persistent_command(
    args: &RunCommandArgs,
    ctx: &ToolContext,
    exec_session_id: ExecSessionId,
) -> Result<CommandOutcome, String> {
    let thread_id = ctx
        .thread_id
        .as_ref()
        .ok_or_else(|| "persistent exec sessions require a runtime thread_id".to_string())?;

    let snapshot = if args.terminate.unwrap_or(false) {
        ctx.exec_sessions
            .terminate(thread_id, &exec_session_id)
            .await?
    } else if let Some(stdin) = &args.stdin {
        ctx.exec_sessions
            .write_stdin(thread_id, &exec_session_id, stdin)
            .await?
    } else {
        ctx.exec_sessions.poll(thread_id, &exec_session_id).await?
    };

    Ok(persistent_outcome(snapshot, ctx.config.max_output_bytes))
}

fn persistent_outcome(
    snapshot: crate::exec_session::ExecSessionSnapshot,
    max_output_bytes: usize,
) -> CommandOutcome {
    let status = match snapshot.status {
        ExecSessionStatus::Exited if snapshot.exit_code.unwrap_or_default() != 0 => {
            ToolStatus::Error
        }
        _ => ToolStatus::Success,
    };
    let exec_session_id = snapshot.exec_session_id.clone();
    let command = snapshot.command.clone();
    let cwd = snapshot.cwd.clone();
    let lifecycle = exec_lifecycle(&snapshot.status);
    let effect = if matches!(snapshot.status, ExecSessionStatus::Running) {
        ToolRuntimeEffect::ExecSessionRunning {
            exec_session_id: exec_session_id.clone(),
            command: command.clone(),
            cwd: cwd.clone(),
        }
    } else {
        ToolRuntimeEffect::ExecSessionNotRunning {
            exec_session_id: exec_session_id.clone(),
        }
    };
    let stdout = project_output(snapshot.stdout.as_bytes(), max_output_bytes);
    let stderr = project_output(snapshot.stderr.as_bytes(), max_output_bytes);
    let stdout_delta = project_output(snapshot.stdout_delta.as_bytes(), max_output_bytes);
    let stderr_delta = project_output(snapshot.stderr_delta.as_bytes(), max_output_bytes);
    let stdout_content = stdout.content;
    let stderr_content = stderr.content;
    let stdout_delta_content = stdout_delta.content;
    let stderr_delta_content = stderr_delta.content;
    let stdout_truncated = stdout.truncated;
    let stderr_truncated = stderr.truncated;
    let stdout_delta_truncated = stdout_delta.truncated;
    let stderr_delta_truncated = stderr_delta.truncated;

    CommandOutcome {
        status,
        content: format!(
            "lifecycle: {}\n\nstdout_delta:\n{}\n\nstderr_delta:\n{}\n\nstdout_projection:\n{}\n\nstderr_projection:\n{}",
            lifecycle,
            stdout_delta_content,
            stderr_delta_content,
            stdout_content,
            stderr_content
        ),
        meta: json!({
            "exec_session_id": exec_session_id.as_str(),
            "command": command,
            "cwd": cwd,
            "lifecycle": lifecycle,
            "stdout": stdout_content,
            "stderr": stderr_content,
            "stdout_delta": stdout_delta_content,
            "stderr_delta": stderr_delta_content,
            "stdout_bytes": snapshot.stdout_bytes,
            "stderr_bytes": snapshot.stderr_bytes,
            "stdout_delta_bytes": snapshot.stdout_delta_bytes,
            "stderr_delta_bytes": snapshot.stderr_delta_bytes,
            "stdout_truncated": stdout_truncated,
            "stderr_truncated": stderr_truncated,
            "stdout_delta_truncated": stdout_delta_truncated,
            "stderr_delta_truncated": stderr_delta_truncated,
            "output_projection": output_projection_meta(max_output_bytes),
            "output_sequence": snapshot.output_sequence,
            "exit_code": snapshot.exit_code,
            "persistent": true,
        }),
        effects: vec![effect],
    }
}

async fn maybe_require_approval(
    ctx: &ToolContext,
    tool_name: &str,
    command: &str,
    cwd: &PathBuf,
    timeout_secs: Option<u64>,
    persistent: bool,
) -> Result<Option<CommandOutcome>, String> {
    let (decision, reason) = ctx.policy.classify_command(ctx.config.policy_mode, command);

    match decision {
        PolicyDecision::Allow => Ok(None),
        PolicyDecision::Deny => {
            let mut meta = json!({
                "policy_decision": "deny",
                "approval_status": "denied",
                "approval_reason": reason.unwrap_or_else(|| "command denied by policy".to_string()),
            });
            merge_object_meta(&mut meta, permission_profile_meta(ctx));
            Ok(Some(CommandOutcome {
                status: ToolStatus::Error,
                content: "Command denied by policy".into(),
                meta,
                effects: Vec::new(),
            }))
        }
        PolicyDecision::ReviewRequired => {
            let thread_id = ctx
                .thread_id
                .clone()
                .ok_or_else(|| "approval flow requires a runtime thread_id".to_string())?;
            let reason = reason.unwrap_or_else(|| "approval required".to_string());
            let checkpoint_id = create_checkpoint(&ctx.config.workspace_root)
                .map_err(|err| {
                    tracing::warn!(
                        error = %err,
                        workspace_root = %ctx.config.workspace_root.display(),
                        "failed to create workspace checkpoint before approval"
                    );
                    err
                })
                .ok()
                .flatten();
            let approval = ctx
                .policy
                .create_command_approval(
                    thread_id.clone(),
                    tool_name,
                    command,
                    cwd.clone(),
                    timeout_secs,
                    persistent,
                    reason.clone(),
                )
                .await;
            let approval_id = approval.approval_id.clone();
            if let Some(checkpoint_id) = checkpoint_id.clone() {
                ctx.policy
                    .attach_checkpoint_id(&approval_id, checkpoint_id)
                    .await;
            }

            let mut meta = json!({
                    "approval_id": approval_id.as_str(),
                    "approval_status": "pending",
                    "approval_reason": reason,
                    "policy_decision": "review_required",
                    "command": command,
                    "cwd": cwd,
            });
            if let Some(checkpoint_id) = checkpoint_id.as_ref() {
                meta["checkpoint_id"] = json!(checkpoint_id);
            }
            merge_object_meta(&mut meta, permission_profile_meta(ctx));

            Ok(Some(CommandOutcome {
                status: ToolStatus::ReviewRequired,
                content: format!("Command requires approval: {}", reason),
                meta,
                effects: vec![ToolRuntimeEffect::ApprovalRequested {
                    approval_id,
                    tool_name: tool_name.to_string(),
                    reason,
                    checkpoint_id,
                    permission_profile: ctx.config.permission_profile,
                    filesystem_sandbox: "none".to_string(),
                    network_sandbox: "none".to_string(),
                    env_isolation: "none".to_string(),
                    command: Some(ApprovalCommandPayload {
                        command: command.to_string(),
                        cwd: cwd.to_string_lossy().into_owned(),
                        timeout_secs,
                        persistent,
                    }),
                }],
            }))
        }
    }
}

async fn run_one_shot_command(
    command_text: &str,
    cwd: PathBuf,
    timeout_secs: u64,
    ctx: &ToolContext,
) -> Result<CommandOutcome, String> {
    let started_at = tokio::time::Instant::now();
    let mut command = Command::new("sh");
    command.arg("-lc").arg(command_text);
    command.current_dir(&cwd);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);
    configure_process_group(&mut command);

    let mut child = command.spawn().map_err(|err| err.to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture stderr".to_string())?;
    let stdout_task = tokio::spawn(read_output_pipe(stdout));
    let stderr_task = tokio::spawn(read_output_pipe(stderr));
    let wait = timeout(Duration::from_secs(timeout_secs), child.wait()).await;

    match wait {
        Ok(Ok(status_result)) => {
            let stdout_bytes = join_output_task(stdout_task).await?;
            let stderr_bytes = join_output_task(stderr_task).await?;
            let stdout = project_output(&stdout_bytes, ctx.config.max_output_bytes);
            let stderr = project_output(&stderr_bytes, ctx.config.max_output_bytes);
            let status = if status_result.success() {
                ToolStatus::Success
            } else {
                ToolStatus::Error
            };
            let stdout_original_bytes = stdout.original_bytes;
            let stderr_original_bytes = stderr.original_bytes;
            let stdout_truncated = stdout.truncated;
            let stderr_truncated = stderr.truncated;
            let stdout_content = stdout.content;
            let stderr_content = stderr.content;
            let content = format!(
                "stdout:\n{}\n\nstderr:\n{}",
                &stdout_content, &stderr_content
            );
            let mut meta = json!({
                "command": command_text,
                "exit_code": status_result.code(),
                "stdout": stdout_content,
                "stderr": stderr_content,
                "stdout_bytes": stdout_original_bytes,
                "stderr_bytes": stderr_original_bytes,
                "stdout_truncated": stdout_truncated,
                "stderr_truncated": stderr_truncated,
                "output_projection": output_projection_meta(ctx.config.max_output_bytes),
                "timed_out": false,
                "duration_ms": elapsed_millis(started_at),
                "cwd": cwd,
            });
            merge_object_meta(&mut meta, permission_profile_meta(ctx));

            Ok(CommandOutcome {
                status,
                content,
                meta,
                effects: Vec::new(),
            })
        }
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => {
            let cleanup = cleanup_child_process_tree(
                &mut child,
                ProcessCleanupReason::Timeout,
                Duration::from_millis(750),
            )
            .await;
            let stdout_bytes = join_output_task(stdout_task).await.unwrap_or_default();
            let stderr_bytes = join_output_task(stderr_task).await.unwrap_or_default();
            let stdout = project_output(&stdout_bytes, ctx.config.max_output_bytes);
            let stderr = project_output(&stderr_bytes, ctx.config.max_output_bytes);
            let duration_ms = elapsed_millis(started_at);
            let stdout_original_bytes = stdout.original_bytes;
            let stderr_original_bytes = stderr.original_bytes;
            let stdout_truncated = stdout.truncated;
            let stderr_truncated = stderr.truncated;
            let stdout_content = stdout.content;
            let stderr_content = stderr.content;
            let content = format!(
                "Command timed out\n\nstdout:\n{}\n\nstderr:\n{}",
                &stdout_content, &stderr_content
            );
            let mut meta = json!({
                "command": command_text,
                "exit_code": Value::Null,
                "stdout": stdout_content,
                "stderr": stderr_content,
                "stdout_bytes": stdout_original_bytes,
                "stderr_bytes": stderr_original_bytes,
                "stdout_truncated": stdout_truncated,
                "stderr_truncated": stderr_truncated,
                "output_projection": output_projection_meta(ctx.config.max_output_bytes),
                "timed_out": true,
                "duration_ms": duration_ms,
                "cwd": cwd,
                "cleanup": cleanup,
            });
            merge_object_meta(&mut meta, permission_profile_meta(ctx));

            Ok(CommandOutcome {
                status: ToolStatus::Error,
                content,
                meta,
                effects: Vec::new(),
            })
        }
    }
}

fn elapsed_millis(started_at: tokio::time::Instant) -> u64 {
    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

async fn read_output_pipe<R>(mut reader: R) -> std::io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut output = Vec::new();
    reader.read_to_end(&mut output).await?;
    Ok(output)
}

async fn join_output_task(
    task: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
) -> Result<Vec<u8>, String> {
    task.await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())
}

fn resolve_cwd(args: &RunCommandArgs, ctx: &ToolContext) -> Result<PathBuf, String> {
    match &args.cwd {
        Some(raw) => resolve_workspace_path(&ctx.config.workspace_root, raw)
            .map(|resolved| resolved.canonical_path)
            .map_err(|err| err.to_string()),
        None => Ok(ctx.config.cwd.clone()),
    }
}

fn exec_lifecycle(status: &ExecSessionStatus) -> &'static str {
    match status {
        ExecSessionStatus::Running => "running",
        ExecSessionStatus::Exited => "exited",
        ExecSessionStatus::Terminated => "terminated",
    }
}

fn annotate_policy_meta(
    meta: &mut Value,
    approval_id: &ApprovalId,
    approval_status: ApprovalStatus,
    policy_decision: &str,
    reason: Option<&str>,
) {
    if let Some(object) = meta.as_object_mut() {
        object.insert(
            "approval_id".into(),
            Value::String(approval_id.as_str().into()),
        );
        object.insert(
            "approval_status".into(),
            Value::String(
                match approval_status {
                    ApprovalStatus::Pending => "pending",
                    ApprovalStatus::Approved => "approved",
                    ApprovalStatus::Denied => "denied",
                }
                .into(),
            ),
        );
        object.insert(
            "policy_decision".into(),
            Value::String(policy_decision.to_string()),
        );
        if let Some(reason) = reason {
            object.insert("approval_reason".into(), Value::String(reason.to_string()));
        }
    }
}

fn annotate_checkpoint_meta(meta: &mut Value, checkpoint_id: Option<&str>) {
    if let (Some(object), Some(checkpoint_id)) = (meta.as_object_mut(), checkpoint_id) {
        object.insert(
            "checkpoint_id".into(),
            Value::String(checkpoint_id.to_string()),
        );
    }
}

fn permission_profile_meta(ctx: &ToolContext) -> Value {
    json!({
        "permission_profile": ctx.config.permission_profile.as_str(),
        "filesystem_sandbox": "none",
        "network_sandbox": "none",
        "env_isolation": "none",
    })
}

fn merge_object_meta(target: &mut Value, extra: Value) {
    let Some(target_object) = target.as_object_mut() else {
        return;
    };
    let Some(extra_object) = extra.as_object() else {
        return;
    };
    for (key, value) in extra_object {
        target_object.insert(key.clone(), value.clone());
    }
}
