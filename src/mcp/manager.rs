use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use tokio::sync::RwLock;

use crate::mcp::client::{
    McpCallOutput, McpClient, McpClientFactory, McpToolDefinition, RmcpClientFactory,
};
use crate::mcp::config::McpServerConfig;
use crate::mcp::tool::McpToolHandler;
use crate::tools::ToolSpec;

#[derive(Debug, Clone, PartialEq)]
pub struct McpDiscoveredTool {
    pub server_id: String,
    pub tool_name: String,
    pub spec: ToolSpec,
}

struct McpServerState {
    config: McpServerConfig,
    client: Option<Arc<dyn McpClient>>,
    tools: Vec<McpDiscoveredTool>,
    last_error: Option<String>,
}

pub struct McpRuntimeManager {
    default_cwd: PathBuf,
    factory: Arc<dyn McpClientFactory>,
    servers: RwLock<HashMap<String, McpServerState>>,
}

impl McpRuntimeManager {
    pub fn new(servers: Vec<McpServerConfig>, default_cwd: PathBuf) -> Self {
        Self::with_factory(servers, default_cwd, Arc::new(RmcpClientFactory))
    }

    pub fn with_factory(
        servers: Vec<McpServerConfig>,
        default_cwd: PathBuf,
        factory: Arc<dyn McpClientFactory>,
    ) -> Self {
        let servers = servers
            .into_iter()
            .map(|mut config| {
                config.id = McpServerConfig::normalized_id(&config.id);
                let id = config.id.clone();
                (
                    id,
                    McpServerState {
                        config,
                        client: None,
                        tools: Vec::new(),
                        last_error: None,
                    },
                )
            })
            .collect();

        Self {
            default_cwd,
            factory,
            servers: RwLock::new(servers),
        }
    }

    pub async fn refresh_tools(&self) -> Result<Vec<McpDiscoveredTool>> {
        let server_ids = self
            .servers
            .read()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut all_tools = Vec::new();
        let mut used_visible_names = HashSet::new();

        for server_id in server_ids {
            match self
                .refresh_server(&server_id, &mut used_visible_names)
                .await
            {
                Ok(mut tools) => all_tools.append(&mut tools),
                Err(err) => {
                    self.record_refresh_error(&server_id, &err).await;
                    tracing::warn!("failed to refresh MCP server {server_id}: {err:#}");
                }
            }
        }

        Ok(all_tools)
    }

    pub async fn handlers(self: &Arc<Self>) -> Result<Vec<McpToolHandler>> {
        self.refresh_tools().await.map(|tools| {
            tools
                .into_iter()
                .map(|tool| McpToolHandler::new(tool, self.clone()))
                .collect()
        })
    }

    pub async fn call_tool(
        &self,
        server_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpCallOutput> {
        let server_id = McpServerConfig::normalized_id(server_id);
        let client = {
            let servers = self.servers.read().await;
            servers
                .get(&server_id)
                .and_then(|state| state.client.clone())
                .ok_or_else(|| anyhow!("MCP server `{server_id}` is not connected"))?
        };

        client
            .call_tool(tool_name, arguments)
            .await
            .with_context(|| format!("MCP tool call failed for `{server_id}/{tool_name}`"))
    }

    pub async fn shutdown(&self) {
        let clients = {
            let mut servers = self.servers.write().await;
            servers
                .values_mut()
                .filter_map(|state| state.client.take())
                .collect::<Vec<_>>()
        };

        for client in clients {
            if let Err(err) = client.shutdown().await {
                tracing::warn!("failed to shutdown MCP client: {err:#}");
            }
        }
    }

    async fn refresh_server(
        &self,
        server_id: &str,
        used_visible_names: &mut HashSet<String>,
    ) -> Result<Vec<McpDiscoveredTool>> {
        let (config, client) = self.connected_client(server_id).await?;
        let definitions = client
            .list_tools()
            .await
            .with_context(|| format!("failed to list tools for MCP server `{server_id}`"))?;
        let tools = discovered_tools(&config, definitions, used_visible_names);

        let mut servers = self.servers.write().await;
        if let Some(state) = servers.get_mut(server_id) {
            state.tools = tools.clone();
            state.last_error = None;
        }

        Ok(tools)
    }

    async fn connected_client(
        &self,
        server_id: &str,
    ) -> Result<(McpServerConfig, Arc<dyn McpClient>)> {
        let (config, existing_client) = {
            let servers = self.servers.read().await;
            let state = servers
                .get(server_id)
                .ok_or_else(|| anyhow!("unknown MCP server `{server_id}`"))?;
            (state.config.clone(), state.client.clone())
        };

        if let Some(client) = existing_client {
            return Ok((config, client));
        }

        let client = self
            .factory
            .connect(&config, &self.default_cwd)
            .await
            .with_context(|| format!("failed to connect MCP server `{server_id}`"))?;

        let installed = {
            let mut servers = self.servers.write().await;
            match servers.get_mut(server_id) {
                Some(state) => {
                    if let Some(existing_client) = state.client.clone() {
                        Ok((state.config.clone(), existing_client))
                    } else {
                        state.client = Some(client.clone());
                        return Ok((state.config.clone(), client));
                    }
                }
                None => Err(anyhow!("unknown MCP server `{server_id}`")),
            }
        };

        if let Err(err) = client.shutdown().await {
            tracing::warn!("failed to shutdown duplicate MCP client for {server_id}: {err:#}");
        }

        installed
    }

    async fn record_refresh_error(&self, server_id: &str, err: &anyhow::Error) {
        let mut servers = self.servers.write().await;
        if let Some(state) = servers.get_mut(server_id) {
            state.last_error = Some(format!("{err:#}"));
        }
    }
}

fn discovered_tools(
    config: &McpServerConfig,
    definitions: Vec<McpToolDefinition>,
    used_visible_names: &mut HashSet<String>,
) -> Vec<McpDiscoveredTool> {
    definitions
        .into_iter()
        .enumerate()
        .map(|(index, definition)| discovered_tool(config, definition, index, used_visible_names))
        .collect()
}

fn discovered_tool(
    config: &McpServerConfig,
    definition: McpToolDefinition,
    index: usize,
    used_visible_names: &mut HashSet<String>,
) -> McpDiscoveredTool {
    let visible_name = visible_tool_name(&config.id, &definition.name, index, used_visible_names);

    McpDiscoveredTool {
        server_id: config.id.clone(),
        tool_name: definition.name,
        spec: ToolSpec::function(
            visible_name,
            definition.description,
            definition.input_schema,
        ),
    }
}

fn visible_tool_name(
    server_id: &str,
    remote_tool_name: &str,
    index: usize,
    used_visible_names: &mut HashSet<String>,
) -> String {
    const MAX_VISIBLE_NAME_LEN: usize = 64;
    const SERVER_COMPONENT_CAP: usize = 24;

    let server = normalized_component(server_id, "server");
    let tool = normalized_component(remote_tool_name, "tool");
    let unsuffixed = format!("mcp__{server}__{tool}");
    if unsuffixed.len() <= MAX_VISIBLE_NAME_LEN && used_visible_names.insert(unsuffixed.clone()) {
        return unsuffixed;
    }

    for attempt in 0.. {
        let suffix = format!(
            "_{:08x}",
            stable_hash32(server_id, remote_tool_name, index, attempt)
        );
        let component_budget = MAX_VISIBLE_NAME_LEN - "mcp__".len() - "__".len() - suffix.len();
        let server_budget = component_budget
            .saturating_sub(1)
            .min(SERVER_COMPONENT_CAP)
            .max(1);
        let visible_server = truncate_ascii(&server, server_budget);
        let tool_budget = component_budget - visible_server.len();
        let visible_tool = truncate_ascii(&tool, tool_budget.max(1));
        let candidate = format!("mcp__{visible_server}__{visible_tool}{suffix}");

        if candidate.len() <= MAX_VISIBLE_NAME_LEN && used_visible_names.insert(candidate.clone()) {
            return candidate;
        }
    }

    unreachable!("visible MCP tool name allocation is unbounded")
}

fn normalized_component(value: &str, fallback: &str) -> String {
    let normalized = McpServerConfig::normalized_id(value);
    if normalized.is_empty() {
        fallback.to_string()
    } else {
        normalized
    }
}

fn truncate_ascii(value: &str, max_len: usize) -> String {
    value.chars().take(max_len).collect()
}

fn stable_hash32(server_id: &str, remote_tool_name: &str, index: usize, attempt: usize) -> u32 {
    let mut hash = 0x811c9dc5u32;
    for byte in server_id
        .bytes()
        .chain([0])
        .chain(remote_tool_name.bytes())
        .chain([0])
        .chain(index.to_le_bytes())
        .chain([0])
        .chain(attempt.to_le_bytes())
    {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}
