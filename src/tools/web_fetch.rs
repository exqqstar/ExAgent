use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::time::Instant;

use crate::events::ApprovalCommandPayload;
use crate::policy::PolicyMode;
use crate::registry::ToolContext;
use crate::session::{ApprovalId, ApprovalStatus};
use crate::tools::output_projection::{output_projection_meta, project_output};
use crate::tools::{
    Tool, ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolRuntimeEffect, ToolSpec,
};
use crate::types::{ToolCall, ToolResult, ToolStatus};

const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 120;
const MAX_REDIRECTS: usize = 5;
const USER_AGENT: &str = "ExAgent";
const RATE_LIMIT_MAX_REQUESTS: usize = 10;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

static RATE_LIMITER: LazyLock<HostRateLimiter> = LazyLock::new(HostRateLimiter::default);

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WebFetchArgs {
    pub url: Option<String>,
    pub timeout_secs: Option<u64>,
    pub approval_id: Option<String>,
    pub decision: Option<String>,
}

pub struct WebFetchTool;

#[async_trait]
impl ToolHandler for WebFetchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "web_fetch",
            "Fetch a URL and return its readable text content (requires approval)",
            serde_json::to_value(schemars::schema_for!(WebFetchArgs)).unwrap(),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            mutating: false,
            requires_approval: true,
            parallel_safe: true,
        }
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args = match serde_json::from_value::<WebFetchArgs>(call.arguments) {
            Ok(args) => args,
            Err(err) => return web_fetch_error(call.id, call.name, err.to_string()),
        };

        match handle_web_fetch(&args, ctx).await {
            Ok(result) => ToolOutcome::from_result(ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: result.status,
                content: result.content,
                meta: Some(result.meta),
                parts: Vec::new(),
            })
            .with_effects(result.effects),
            Err(err) => web_fetch_error(call.id, call.name, err),
        }
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &'static str {
        "web_fetch"
    }

    fn description(&self) -> &'static str {
        "Fetch a URL and return its readable text content (requires approval)"
    }

    fn input_schema(&self) -> Value {
        serde_json::to_value(schemars::schema_for!(WebFetchArgs)).unwrap()
    }

    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> ToolResult {
        let invocation = ToolInvocation {
            invocation_id: format!("inv_{}", call.id),
            call,
        };
        self.handle(invocation, ctx).await.model_result
    }
}

#[derive(Debug, Clone, PartialEq)]
struct WebFetchOutcome {
    status: ToolStatus,
    content: String,
    meta: Value,
    effects: Vec<ToolRuntimeEffect>,
}

async fn handle_web_fetch(
    args: &WebFetchArgs,
    ctx: &ToolContext,
) -> Result<WebFetchOutcome, String> {
    if let Some(approval_id) = &args.approval_id {
        return handle_approval_decision(args, ctx, ApprovalId::new(approval_id)).await;
    }

    let url = args
        .url
        .as_deref()
        .ok_or_else(|| "url is required".to_string())?;
    let parsed = validate_http_url(url)?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "url must include a hostname".to_string())?;
    RATE_LIMITER.check(host)?;
    let timeout_secs = normalize_timeout(args.timeout_secs);

    if matches!(ctx.config.policy_mode, PolicyMode::Off) {
        return fetch_url(parsed.as_str(), timeout_secs, ctx).await;
    }

    request_approval(parsed.as_str(), args.timeout_secs, ctx).await
}

async fn request_approval(
    url: &str,
    timeout_secs: Option<u64>,
    ctx: &ToolContext,
) -> Result<WebFetchOutcome, String> {
    let thread_id = ctx
        .thread_id
        .clone()
        .ok_or_else(|| "approval flow requires a runtime thread_id".to_string())?;
    let reason = "network fetch requires approval".to_string();
    let approval = ctx
        .policy
        .create_command_approval(
            thread_id,
            "web_fetch",
            url,
            ctx.config.workspace_root.clone(),
            timeout_secs,
            false,
            reason.clone(),
        )
        .await;
    let approval_id = approval.approval_id.clone();
    let cwd = ctx.config.workspace_root.to_string_lossy().into_owned();
    let mut meta = json!({
        "approval_id": approval_id.as_str(),
        "approval_status": "pending",
        "approval_reason": reason,
        "policy_decision": "review_required",
        "url": url,
        "command": url,
        "cwd": ctx.config.workspace_root,
    });
    merge_object_meta(&mut meta, permission_profile_meta(ctx));

    Ok(WebFetchOutcome {
        status: ToolStatus::ReviewRequired,
        content: format!("Fetch requires approval: {}", reason),
        meta,
        effects: vec![ToolRuntimeEffect::ApprovalRequested {
            approval_id,
            tool_name: "web_fetch".to_string(),
            reason,
            checkpoint_id: None,
            permission_profile: ctx.config.permission_profile,
            filesystem_sandbox: "none".to_string(),
            network_sandbox: "none".to_string(),
            env_isolation: "none".to_string(),
            command: Some(ApprovalCommandPayload {
                command: url.to_string(),
                cwd,
                timeout_secs,
                persistent: false,
            }),
        }],
    })
}

async fn handle_approval_decision(
    args: &WebFetchArgs,
    ctx: &ToolContext,
    approval_id: ApprovalId,
) -> Result<WebFetchOutcome, String> {
    let decision = args
        .decision
        .as_deref()
        .ok_or_else(|| "decision is required when approval_id is provided".to_string())?;
    let pending = ctx.policy.take_pending_command(&approval_id).await?;

    match decision {
        "approved" => {
            let timeout_secs = normalize_timeout(pending.timeout_secs);
            let mut outcome = fetch_url(&pending.command, timeout_secs, ctx).await?;
            annotate_policy_meta(
                &mut outcome.meta,
                &approval_id,
                ApprovalStatus::Approved,
                "allow",
                Some(pending.reason.as_str()),
            );
            outcome.effects.push(ToolRuntimeEffect::ApprovalApproved {
                approval_id,
                note: None,
            });
            Ok(outcome)
        }
        "denied" => {
            let mut meta = json!({
                "approval_id": approval_id.as_str(),
                "approval_status": "denied",
                "policy_decision": "deny",
                "approval_reason": pending.reason,
                "url": pending.command,
            });
            merge_object_meta(&mut meta, permission_profile_meta(ctx));
            Ok(WebFetchOutcome {
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

async fn fetch_url(
    url: &str,
    timeout_secs: u64,
    ctx: &ToolContext,
) -> Result<WebFetchOutcome, String> {
    let started_at = Instant::now();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|err| err.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let final_url = response.url().to_string();
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let (body, body_truncated) = read_capped_body(response).await?;
    let body_bytes = body.len();
    let extracted = extract_text(&body, content_type.as_deref())?;
    let output = project_output(extracted.as_bytes(), ctx.config.max_output_bytes);
    let output_truncated = output.truncated;
    let output_content = output.content;
    let content_type_meta = content_type.unwrap_or_else(|| "unknown".to_string());

    let mut meta = json!({
        "url": url,
        "final_url": final_url,
        "http_status": status.as_u16(),
        "content_type": content_type_meta,
        "body_bytes": body_bytes,
        "body_truncated": body_truncated,
        "output_truncated": output_truncated,
        "duration_ms": elapsed_millis(started_at),
        "output_projection": output_projection_meta(ctx.config.max_output_bytes),
    });
    merge_object_meta(&mut meta, permission_profile_meta(ctx));

    Ok(WebFetchOutcome {
        status: if status.is_success() {
            ToolStatus::Success
        } else {
            ToolStatus::Error
        },
        content: output_content,
        meta,
        effects: Vec::new(),
    })
}

async fn read_capped_body(response: reqwest::Response) -> Result<(Vec<u8>, bool), String> {
    let mut body = Vec::new();
    let mut truncated = false;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| err.to_string())?;
        if body.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
            let remaining = MAX_BODY_BYTES.saturating_sub(body.len());
            body.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
    }

    Ok((body, truncated))
}

fn extract_text(body: &[u8], content_type: Option<&str>) -> Result<String, String> {
    let content_type = content_type.unwrap_or("unknown");
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    let raw_text = String::from_utf8_lossy(body).to_string();

    if media_type == "text/html" {
        return htmd::HtmlToMarkdown::builder()
            .skip_tags(vec!["script", "style"])
            .build()
            .convert(&raw_text)
            .map_err(|err| err.to_string());
    }

    if media_type.starts_with("text/")
        || media_type == "application/json"
        || media_type.ends_with("+json")
        || media_type == "application/xml"
        || media_type.ends_with("+xml")
    {
        return Ok(raw_text);
    }

    Err(format!("unsupported content type: {content_type}"))
}

fn validate_http_url(url: &str) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(url).map_err(|err| err.to_string())?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        _ => Err("url scheme must be http or https".to_string()),
    }
}

fn normalize_timeout(timeout_secs: Option<u64>) -> u64 {
    timeout_secs
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(1, MAX_TIMEOUT_SECS)
}

fn elapsed_millis(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
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

#[derive(Debug, Default)]
struct HostRateLimiter {
    requests: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl HostRateLimiter {
    fn check(&self, host: &str) -> Result<(), String> {
        self.check_at(host, Instant::now()).map_err(|retry_after| {
            format!(
                "rate limited for {host}; retry in {}s",
                retry_after.as_secs().max(1)
            )
        })
    }

    fn check_at(&self, host: &str, now: Instant) -> Result<(), Duration> {
        let mut requests = self.requests.lock().expect("rate limit mutex");
        let entries = requests.entry(host.to_string()).or_default();
        while let Some(front) = entries.front() {
            let Some(age) = now.checked_duration_since(*front) else {
                break;
            };
            if age < RATE_LIMIT_WINDOW {
                break;
            }
            entries.pop_front();
        }

        if entries.len() >= RATE_LIMIT_MAX_REQUESTS {
            let retry_after = entries
                .front()
                .and_then(|front| {
                    now.checked_duration_since(*front)
                        .map(|age| RATE_LIMIT_WINDOW.saturating_sub(age))
                })
                .unwrap_or(RATE_LIMIT_WINDOW);
            return Err(retry_after);
        }

        entries.push_back(now);
        Ok(())
    }
}

fn web_fetch_error(tool_call_id: String, tool_name: String, content: String) -> ToolOutcome {
    ToolOutcome::from_result(ToolResult {
        tool_call_id,
        tool_name,
        status: ToolStatus::Error,
        content,
        meta: None,
        parts: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::{HostRateLimiter, RATE_LIMIT_MAX_REQUESTS, RATE_LIMIT_WINDOW};
    use tokio::time::Instant;

    #[test]
    fn rate_limiter_caps_requests_per_host_in_sliding_window() {
        let limiter = HostRateLimiter::default();
        let now = Instant::now();

        for _ in 0..RATE_LIMIT_MAX_REQUESTS {
            assert!(limiter.check_at("example.com", now).is_ok());
        }
        let retry_after = limiter
            .check_at("example.com", now + std::time::Duration::from_secs(1))
            .unwrap_err();
        assert!(retry_after.as_secs() >= 58);

        assert!(limiter
            .check_at("example.com", now + RATE_LIMIT_WINDOW)
            .is_ok());
        assert!(limiter
            .check_at("other.example", now + std::time::Duration::from_secs(1))
            .is_ok());
    }
}
