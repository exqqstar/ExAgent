use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, ClientInfo};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::child_process::TokioChildProcess;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::mcp::config::McpServerConfig;

#[derive(Debug, Clone, PartialEq)]
pub struct McpToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpCallOutput {
    pub content: Vec<Value>,
    pub is_error: bool,
    pub meta: Option<Value>,
}

#[async_trait]
pub trait McpClient: Send + Sync {
    async fn list_tools(&self) -> anyhow::Result<Vec<McpToolDefinition>>;
    async fn call_tool(&self, name: &str, arguments: Value) -> anyhow::Result<McpCallOutput>;
    async fn shutdown(&self) -> anyhow::Result<()>;
}

#[async_trait]
pub trait McpClientFactory: Send + Sync {
    async fn connect(
        &self,
        server: &McpServerConfig,
        default_cwd: &Path,
    ) -> anyhow::Result<Arc<dyn McpClient>>;
}

pub struct RmcpClientFactory;

struct RmcpClient {
    service: Mutex<RunningService<RoleClient, ClientInfo>>,
}

#[async_trait]
impl McpClientFactory for RmcpClientFactory {
    async fn connect(
        &self,
        server: &McpServerConfig,
        default_cwd: &Path,
    ) -> anyhow::Result<Arc<dyn McpClient>> {
        let command = server.command.trim();
        if command.is_empty() {
            return Err(anyhow!("MCP server `{}` command is empty", server.id));
        }

        let mut process = Command::new(command);
        process
            .args(&server.args)
            .current_dir(server.working_directory.as_deref().unwrap_or(default_cwd))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(&server.env);

        let (transport, stderr) = TokioChildProcess::builder(process)
            .spawn()
            .with_context(|| format!("failed to spawn MCP server `{}`", server.id))?;

        if let Some(stderr) = stderr {
            let server_id = server.id.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                loop {
                    match lines.next_line().await {
                        Ok(Some(line)) => tracing::info!("MCP server stderr ({server_id}): {line}"),
                        Ok(None) => break,
                        Err(err) => {
                            tracing::warn!("failed reading MCP server stderr ({server_id}): {err}");
                            break;
                        }
                    }
                }
            });
        }

        let client_info = ClientInfo::default();
        let service = rmcp::serve_client(client_info, transport)
            .await
            .with_context(|| format!("failed to initialize MCP server `{}`", server.id))?;

        Ok(Arc::new(RmcpClient {
            service: Mutex::new(service),
        }))
    }
}

#[async_trait]
impl McpClient for RmcpClient {
    async fn list_tools(&self) -> anyhow::Result<Vec<McpToolDefinition>> {
        let service = self.service.lock().await;
        let tools = service
            .peer()
            .list_all_tools()
            .await
            .context("failed to list MCP tools")?;

        Ok(tools
            .into_iter()
            .map(|tool| McpToolDefinition {
                name: tool.name.into_owned(),
                description: tool
                    .description
                    .map(|value| value.into_owned())
                    .unwrap_or_default(),
                input_schema: Value::Object((*tool.input_schema).clone()),
            })
            .collect())
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> anyhow::Result<McpCallOutput> {
        let arguments = match arguments {
            Value::Object(map) => Some(map),
            Value::Null => None,
            other => {
                return Err(anyhow!(
                    "MCP tool arguments must be a JSON object or null, got {other}"
                ));
            }
        };
        let params = CallToolRequestParams {
            meta: None,
            name: name.to_string().into(),
            arguments,
            task: None,
        };

        let service = self.service.lock().await;
        let result = service
            .peer()
            .call_tool(params)
            .await
            .with_context(|| format!("failed to call MCP tool `{name}`"))?;

        let content = result
            .content
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .context("failed to convert MCP tool content")?;
        Ok(McpCallOutput {
            content,
            is_error: result.is_error.unwrap_or(false),
            meta: result.meta.map(|meta| Value::Object(meta.0)),
        })
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        let mut service = self.service.lock().await;
        service
            .close()
            .await
            .context("failed to close MCP client")?;
        Ok(())
    }
}
