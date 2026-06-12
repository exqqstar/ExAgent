use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use async_trait::async_trait;
use exagent::mcp::client::{McpCallOutput, McpClient, McpClientFactory, McpToolDefinition};
use exagent::mcp::config::McpServerConfig;
use exagent::mcp::manager::McpRuntimeManager;
use exagent::mcp::tool::McpToolHandler;
use exagent::registry::ToolContext;
use exagent::tools::{ToolCapabilities, ToolHandler, ToolInvocation};
use exagent::types::{ToolCall, ToolStatus};
use serde_json::json;
use tokio::sync::Barrier;

#[derive(Clone, Default)]
struct FakeFactory {
    calls: Arc<Mutex<Vec<String>>>,
    failed_servers: Arc<Mutex<HashSet<String>>>,
}

struct FakeClient {
    server_id: String,
    calls: Arc<Mutex<Vec<String>>>,
    fail_list_tools: bool,
}

#[async_trait]
impl McpClient for FakeClient {
    async fn list_tools(&self) -> anyhow::Result<Vec<McpToolDefinition>> {
        if self.fail_list_tools {
            return Err(anyhow!("simulated list failure"));
        }

        Ok(vec![McpToolDefinition {
            name: "lookup".into(),
            description: "Lookup a record.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
        }])
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> anyhow::Result<McpCallOutput> {
        self.calls.lock().unwrap().push(format!(
            "{}:{}:{}",
            self.server_id,
            name,
            arguments["query"].as_str().unwrap()
        ));
        Ok(McpCallOutput {
            content: vec![json!({ "type": "text", "text": "found" })],
            is_error: false,
            meta: None,
        })
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[async_trait]
impl McpClientFactory for FakeFactory {
    async fn connect(
        &self,
        server: &McpServerConfig,
        _default_cwd: &Path,
    ) -> anyhow::Result<Arc<dyn McpClient>> {
        Ok(Arc::new(FakeClient {
            server_id: server.id.clone(),
            calls: self.calls.clone(),
            fail_list_tools: self.failed_servers.lock().unwrap().contains(&server.id),
        }))
    }
}

fn server(id: &str) -> McpServerConfig {
    McpServerConfig {
        id: id.into(),
        display_name: id.into(),
        command: "fake".into(),
        args: Vec::new(),
        env: HashMap::new(),
        working_directory: None,
    }
}

#[tokio::test]
async fn manager_discovers_tools_and_dispatches_calls() {
    let factory = FakeFactory::default();
    let manager = McpRuntimeManager::with_factory(
        vec![server("records")],
        std::env::current_dir().unwrap(),
        Arc::new(factory.clone()),
    );

    let tools = manager.refresh_tools().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].server_id, "records");
    assert_eq!(tools[0].tool_name, "lookup");
    assert_eq!(tools[0].spec.name, "mcp__records__lookup");

    let output = manager
        .call_tool("records", "lookup", json!({ "query": "ada" }))
        .await
        .unwrap();
    assert_eq!(output.content[0]["text"], "found");
    assert_eq!(factory.calls.lock().unwrap()[0], "records:lookup:ada");
}

#[tokio::test]
async fn manager_keeps_healthy_tools_when_one_server_refresh_fails() {
    let factory = FakeFactory::default();
    factory
        .failed_servers
        .lock()
        .unwrap()
        .insert("broken".into());
    let manager = McpRuntimeManager::with_factory(
        vec![server("healthy"), server("broken")],
        std::env::current_dir().unwrap(),
        Arc::new(factory),
    );

    let tools = manager.refresh_tools().await.unwrap();

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].server_id, "healthy");
    assert_eq!(tools[0].spec.name, "mcp__healthy__lookup");
}

#[derive(Clone)]
struct RacingFactory {
    barrier: Arc<Barrier>,
    connected: Arc<AtomicUsize>,
    shutdown: Arc<AtomicUsize>,
}

struct RacingClient {
    shutdown: Arc<AtomicUsize>,
}

#[async_trait]
impl McpClient for RacingClient {
    async fn list_tools(&self) -> anyhow::Result<Vec<McpToolDefinition>> {
        Ok(vec![tool_definition("lookup")])
    }

    async fn call_tool(
        &self,
        _name: &str,
        _arguments: serde_json::Value,
    ) -> anyhow::Result<McpCallOutput> {
        Ok(McpCallOutput {
            content: Vec::new(),
            is_error: false,
            meta: None,
        })
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        self.shutdown.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[async_trait]
impl McpClientFactory for RacingFactory {
    async fn connect(
        &self,
        _server: &McpServerConfig,
        _default_cwd: &Path,
    ) -> anyhow::Result<Arc<dyn McpClient>> {
        self.connected.fetch_add(1, Ordering::SeqCst);
        self.barrier.wait().await;
        Ok(Arc::new(RacingClient {
            shutdown: self.shutdown.clone(),
        }))
    }
}

#[tokio::test]
async fn concurrent_first_refresh_shuts_down_losing_duplicate_client() {
    let factory = RacingFactory {
        barrier: Arc::new(Barrier::new(2)),
        connected: Arc::new(AtomicUsize::new(0)),
        shutdown: Arc::new(AtomicUsize::new(0)),
    };
    let manager = Arc::new(McpRuntimeManager::with_factory(
        vec![server("records")],
        std::env::current_dir().unwrap(),
        Arc::new(factory.clone()),
    ));

    let first = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.refresh_tools().await })
    };
    let second = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.refresh_tools().await })
    };

    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();

    assert_eq!(factory.connected.load(Ordering::SeqCst), 2);
    assert_eq!(factory.shutdown.load(Ordering::SeqCst), 1);
}

#[derive(Clone)]
struct ToolListFactory {
    tools: Arc<Vec<McpToolDefinition>>,
}

#[async_trait]
impl McpClientFactory for ToolListFactory {
    async fn connect(
        &self,
        _server: &McpServerConfig,
        _default_cwd: &Path,
    ) -> anyhow::Result<Arc<dyn McpClient>> {
        Ok(Arc::new(ToolListClient {
            tools: self.tools.clone(),
        }))
    }
}

struct ToolListClient {
    tools: Arc<Vec<McpToolDefinition>>,
}

#[async_trait]
impl McpClient for ToolListClient {
    async fn list_tools(&self) -> anyhow::Result<Vec<McpToolDefinition>> {
        Ok((*self.tools).clone())
    }

    async fn call_tool(
        &self,
        _name: &str,
        _arguments: serde_json::Value,
    ) -> anyhow::Result<McpCallOutput> {
        Ok(McpCallOutput {
            content: Vec::new(),
            is_error: false,
            meta: None,
        })
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn visible_tool_names_are_valid_unique_and_bounded() {
    let remote_names = vec![
        "...".to_string(),
        "foo.bar".to_string(),
        "foo/bar".to_string(),
        format!("{}{}", "a".repeat(120), ".tail"),
    ];
    let factory = ToolListFactory {
        tools: Arc::new(
            remote_names
                .iter()
                .map(|name| tool_definition(name))
                .collect(),
        ),
    };
    let manager = McpRuntimeManager::with_factory(
        vec![server("records")],
        std::env::current_dir().unwrap(),
        Arc::new(factory),
    );

    let tools = manager.refresh_tools().await.unwrap();
    let visible_names = tools
        .iter()
        .map(|tool| tool.spec.name.as_str())
        .collect::<Vec<_>>();
    let unique_names = visible_names.iter().copied().collect::<HashSet<_>>();

    assert_eq!(tools.len(), remote_names.len());
    assert_eq!(unique_names.len(), visible_names.len());
    for (tool, remote_name) in tools.iter().zip(remote_names.iter()) {
        assert_eq!(&tool.tool_name, remote_name);
        assert!(!tool.spec.name.is_empty());
        assert!(tool.spec.name.len() <= 64, "{}", tool.spec.name);
        assert!(tool
            .spec
            .name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'));
    }
}

fn tool_definition(name: impl Into<String>) -> McpToolDefinition {
    McpToolDefinition {
        name: name.into(),
        description: "Lookup a record.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"]
        }),
    }
}

fn tool_context() -> ToolContext {
    ToolContext {
        config: exagent::config::AgentConfig::default(),
        thread_id: None,
        turn_id: None,
        tool_invocation_id: None,
        exec_sessions: Arc::new(exagent::exec_session::ExecSessionManager::default()),
        exec_output_sink: None,
        policy: Arc::new(exagent::policy::PolicyManager::default()),
        agent_tool_policy: exagent::runtime::agent_profile::AgentToolPolicy::all(),
        inbox: None,
        goal_api: None,
    }
}

#[tokio::test]
async fn mcp_tool_handler_projects_content_and_calls_remote_tool() {
    let factory = FakeFactory::default();
    let manager = Arc::new(McpRuntimeManager::with_factory(
        vec![server("records")],
        std::env::current_dir().unwrap(),
        Arc::new(factory.clone()),
    ));
    let discovered = manager.refresh_tools().await.unwrap().remove(0);
    let handler = McpToolHandler::new(discovered, manager);

    assert_eq!(handler.spec().name, "mcp__records__lookup");
    assert_eq!(handler.capabilities(), ToolCapabilities::mutating(false));

    let outcome = handler
        .handle(
            ToolInvocation {
                invocation_id: "inv_mcp_1".into(),
                call: ToolCall {
                    id: "call_mcp_1".into(),
                    name: "mcp__records__lookup".into(),
                    arguments: json!({ "query": "ada" }),
                    thought_signature: None,
                },
            },
            &tool_context(),
        )
        .await;

    assert_eq!(outcome.model_result.status, ToolStatus::Success);
    assert_eq!(outcome.model_result.content, "found");
    assert_eq!(factory.calls.lock().unwrap()[0], "records:lookup:ada");
}

#[derive(Clone)]
struct OutputFactory {
    output: Arc<Mutex<anyhow::Result<McpCallOutput>>>,
}

struct OutputClient {
    output: Arc<Mutex<anyhow::Result<McpCallOutput>>>,
}

#[async_trait]
impl McpClientFactory for OutputFactory {
    async fn connect(
        &self,
        _server: &McpServerConfig,
        _default_cwd: &Path,
    ) -> anyhow::Result<Arc<dyn McpClient>> {
        Ok(Arc::new(OutputClient {
            output: self.output.clone(),
        }))
    }
}

#[async_trait]
impl McpClient for OutputClient {
    async fn list_tools(&self) -> anyhow::Result<Vec<McpToolDefinition>> {
        Ok(vec![tool_definition("lookup")])
    }

    async fn call_tool(
        &self,
        _name: &str,
        _arguments: serde_json::Value,
    ) -> anyhow::Result<McpCallOutput> {
        self.output
            .lock()
            .unwrap()
            .as_ref()
            .map(Clone::clone)
            .map_err(|err| anyhow!("{err}"))
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

async fn handle_output(output: anyhow::Result<McpCallOutput>) -> exagent::types::ToolResult {
    let manager = Arc::new(McpRuntimeManager::with_factory(
        vec![server("records")],
        std::env::current_dir().unwrap(),
        Arc::new(OutputFactory {
            output: Arc::new(Mutex::new(output)),
        }),
    ));
    let handler = manager.handlers().await.unwrap().remove(0);

    handler
        .handle(
            ToolInvocation {
                invocation_id: "inv_mcp_output".into(),
                call: ToolCall {
                    id: "call_mcp_output".into(),
                    name: "mcp__records__lookup".into(),
                    arguments: json!({}),
                    thought_signature: None,
                },
            },
            &tool_context(),
        )
        .await
        .model_result
}

#[tokio::test]
async fn mcp_tool_handler_converts_supported_content_items_to_text() {
    let result = handle_output(Ok(McpCallOutput {
        content: vec![
            json!({ "type": "text", "text": "plain text" }),
            json!({ "type": "resource", "resource": { "mimeType": "text/plain", "text": "resource text" } }),
            json!({ "type": "resource", "resource": { "uri": "file:///tmp/report.txt" } }),
            json!({ "type": "image", "mimeType": "image/png", "data": "abc" }),
            json!({ "type": "custom", "value": 7 }),
        ],
        is_error: false,
        meta: None,
    }))
    .await;

    assert_eq!(result.status, ToolStatus::Success);
    assert_eq!(
        result.content,
        "plain text\n\nresource text\n\n[resource: file:///tmp/report.txt]\n\n[image: image/png]\n\n{\"type\":\"custom\",\"value\":7}"
    );
}

#[tokio::test]
async fn mcp_tool_handler_preserves_output_metadata() {
    let result = handle_output(Ok(McpCallOutput {
        content: vec![json!({ "type": "text", "text": "ok" })],
        is_error: false,
        meta: Some(json!({ "cursor": "next", "count": 2 })),
    }))
    .await;

    assert_eq!(result.status, ToolStatus::Success);
    assert_eq!(result.meta, Some(json!({ "cursor": "next", "count": 2 })));
}

#[tokio::test]
async fn mcp_tool_handler_returns_placeholder_for_empty_content() {
    let result = handle_output(Ok(McpCallOutput {
        content: Vec::new(),
        is_error: false,
        meta: None,
    }))
    .await;

    assert_eq!(result.status, ToolStatus::Success);
    assert_eq!(result.content, "[MCP tool returned no content]");
}

#[tokio::test]
async fn mcp_tool_handler_maps_mcp_error_flag_to_error_status() {
    let result = handle_output(Ok(McpCallOutput {
        content: vec![json!({ "type": "text", "text": "remote validation failed" })],
        is_error: true,
        meta: None,
    }))
    .await;

    assert_eq!(result.status, ToolStatus::Error);
    assert_eq!(result.content, "remote validation failed");
}

#[tokio::test]
async fn mcp_tool_handler_maps_call_failure_to_error_result() {
    let result = handle_output(Err(anyhow!("transport closed"))).await;

    assert_eq!(result.status, ToolStatus::Error);
    assert!(result
        .content
        .contains("MCP tool call failed: MCP tool call failed for `records/lookup`"));
    assert!(result.content.contains("transport closed"));
}
