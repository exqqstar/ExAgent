use std::path::PathBuf;
use std::process::Stdio;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::time::{timeout, Duration};

use crate::policy::PolicyDecision;
use crate::registry::ToolContext;
use crate::session::{ApprovalId, ApprovalStatus};
use crate::session::{ExecSessionId, ExecSessionStatus};
use crate::tools::Tool;
use crate::types::{ToolCall, ToolResult, ToolStatus};
use crate::workspace::resolve_workspace_path;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunCommandArgs {
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub timeout_secs: Option<u64>,
    pub persistent: Option<bool>,
    pub exec_session_id: Option<String>,
    pub stdin: Option<String>,
    pub terminate: Option<bool>,
    pub approval_id: Option<String>,
    pub decision: Option<String>,
}

pub struct RunCommandTool;

#[async_trait]
impl Tool for RunCommandTool {
    fn name(&self) -> &'static str {
        "run_command"
    }

    fn description(&self) -> &'static str {
        "Run a shell command inside the workspace"
    }

    fn input_schema(&self) -> Value {
        serde_json::to_value(schemars::schema_for!(RunCommandArgs)).unwrap()
    }

    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> ToolResult {
        let args: RunCommandArgs = match serde_json::from_value(call.arguments) {
            Ok(args) => args,
            Err(err) => {
                return ToolResult {
                    tool_call_id: call.id,
                    tool_name: call.name,
                    status: ToolStatus::Error,
                    content: err.to_string(),
                    meta: None,
                };
            }
        };

        match run_command(&args, ctx).await {
            Ok(result) => ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: result.status,
                content: result.content,
                meta: Some(result.meta),
            },
            Err(err) => ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Error,
                content: err,
                meta: None,
            },
        }
    }
}

struct CommandOutcome {
    status: ToolStatus,
    content: String,
    meta: Value,
}

async fn run_command(args: &RunCommandArgs, ctx: &ToolContext) -> Result<CommandOutcome, String> {
    if let Some(approval_id) = &args.approval_id {
        return handle_approval_decision(args, ctx, ApprovalId::new(approval_id)).await;
    }

    if let Some(exec_session_id) = &args.exec_session_id {
        return run_persistent_command(args, ctx, ExecSessionId::new(exec_session_id)).await;
    }

    if args.persistent.unwrap_or(false) {
        return start_persistent_command(args, ctx).await;
    }

    let command_text = args
        .command
        .as_deref()
        .ok_or_else(|| "command is required".to_string())?;
    let cwd = resolve_cwd(args, ctx)?;
    let timeout_secs = args.timeout_secs.unwrap_or(ctx.config.command_timeout_secs);
    if let Some(outcome) =
        maybe_require_approval(ctx, command_text, &cwd, args.timeout_secs, false).await?
    {
        return Ok(outcome);
    }

    run_one_shot_command(command_text, cwd, timeout_secs, ctx).await
}

async fn start_persistent_command(
    args: &RunCommandArgs,
    ctx: &ToolContext,
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
    if let Some(outcome) = maybe_require_approval(ctx, command, &cwd, None, true).await? {
        return Ok(outcome);
    }
    let snapshot = ctx
        .exec_sessions
        .start(&ctx.config.workspace_root, thread_id, command, cwd)
        .await?;

    Ok(persistent_outcome(snapshot))
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
                        &pending.command,
                        pending.cwd.clone(),
                    )
                    .await?;
                let mut outcome = persistent_outcome(snapshot);
                annotate_policy_meta(
                    &mut outcome.meta,
                    &approval_id,
                    ApprovalStatus::Approved,
                    "allow",
                    Some(pending.reason.as_str()),
                );
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
                Ok(outcome)
            }
        }
        "denied" => Ok(CommandOutcome {
            status: ToolStatus::Error,
            content: "Approval denied".into(),
            meta: json!({
                "approval_id": approval_id.as_str(),
                "approval_status": "denied",
                "policy_decision": "deny",
                "approval_reason": pending.reason,
            }),
        }),
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

    Ok(persistent_outcome(snapshot))
}

fn persistent_outcome(snapshot: crate::exec_session::ExecSessionSnapshot) -> CommandOutcome {
    let status = match snapshot.status {
        ExecSessionStatus::Exited if snapshot.exit_code.unwrap_or_default() != 0 => {
            ToolStatus::Error
        }
        _ => ToolStatus::Success,
    };

    CommandOutcome {
        status,
        content: format!(
            "stdout:\n{}\n\nstderr:\n{}",
            snapshot.stdout, snapshot.stderr
        ),
        meta: json!({
            "exec_session_id": snapshot.exec_session_id.as_str(),
            "command": snapshot.command,
            "cwd": snapshot.cwd,
            "lifecycle": exec_lifecycle(&snapshot.status),
            "stdout": snapshot.stdout,
            "stderr": snapshot.stderr,
            "exit_code": snapshot.exit_code,
            "persistent": true,
        }),
    }
}

async fn maybe_require_approval(
    ctx: &ToolContext,
    command: &str,
    cwd: &PathBuf,
    timeout_secs: Option<u64>,
    persistent: bool,
) -> Result<Option<CommandOutcome>, String> {
    let (decision, reason) = ctx.policy.classify_command(ctx.config.policy_mode, command);

    match decision {
        PolicyDecision::Allow => Ok(None),
        PolicyDecision::Deny => Ok(Some(CommandOutcome {
            status: ToolStatus::Error,
            content: "Command denied by policy".into(),
            meta: json!({
                "policy_decision": "deny",
                "approval_status": "denied",
                "approval_reason": reason.unwrap_or_else(|| "command denied by policy".to_string()),
            }),
        })),
        PolicyDecision::ReviewRequired => {
            let thread_id = ctx
                .thread_id
                .clone()
                .ok_or_else(|| "approval flow requires a runtime thread_id".to_string())?;
            let reason = reason.unwrap_or_else(|| "approval required".to_string());
            let approval = ctx
                .policy
                .create_command_approval(
                    thread_id.clone(),
                    "run_command",
                    command,
                    cwd.clone(),
                    timeout_secs,
                    persistent,
                    reason.clone(),
                )
                .await;

            let meta = json!({
                    "approval_id": approval.approval_id.as_str(),
                    "approval_status": "pending",
                    "approval_reason": reason,
                    "policy_decision": "review_required",
                    "command": command,
                    "cwd": cwd,
            });

            Ok(Some(CommandOutcome {
                status: ToolStatus::ReviewRequired,
                content: format!("Command requires approval: {}", reason),
                meta,
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
    let mut command = Command::new("sh");
    command.arg("-lc").arg(command_text);
    command.current_dir(&cwd);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);

    let child = command.spawn().map_err(|err| err.to_string())?;
    let wait = timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await;

    match wait {
        Ok(Ok(output)) => {
            let stdout = truncate_utf8(
                &String::from_utf8_lossy(&output.stdout),
                ctx.config.max_output_bytes,
            );
            let stderr = truncate_utf8(
                &String::from_utf8_lossy(&output.stderr),
                ctx.config.max_output_bytes,
            );
            let status = if output.status.success() {
                ToolStatus::Success
            } else {
                ToolStatus::Error
            };

            Ok(CommandOutcome {
                status,
                content: format!("stdout:\n{}\n\nstderr:\n{}", stdout, stderr),
                meta: json!({
                    "exit_code": output.status.code(),
                    "stdout": stdout,
                    "stderr": stderr,
                    "timed_out": false,
                    "cwd": cwd,
                }),
            })
        }
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => Ok(CommandOutcome {
            status: ToolStatus::Error,
            content: "Command timed out".into(),
            meta: json!({
                "exit_code": Value::Null,
                "stdout": "",
                "stderr": "",
                "timed_out": true,
                "cwd": cwd,
            }),
        }),
    }
}

fn resolve_cwd(args: &RunCommandArgs, ctx: &ToolContext) -> Result<PathBuf, String> {
    match &args.cwd {
        Some(raw) => {
            resolve_workspace_path(&ctx.config.workspace_root, raw).map_err(|err| err.to_string())
        }
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

fn truncate_utf8(output: &str, max_bytes: usize) -> String {
    if output.len() <= max_bytes {
        return output.to_string();
    }

    let mut end = 0;
    for (idx, ch) in output.char_indices() {
        if idx + ch.len_utf8() > max_bytes {
            break;
        }
        end = idx + ch.len_utf8();
    }
    output[..end].to_string()
}
