# Rust Phase 1 Agent Core Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a minimal Rust coding agent that can loop over LLM responses, execute three local tools, return both tool successes and failures back to the model, and stop when the model emits no tool calls.

**Architecture:** Use a single Rust crate with a thin agent loop, a small tool registry, and a provider-neutral LLM interface. Keep the boundary between the runtime and tools flexible with `serde_json::Value`, but deserialize each tool's arguments into typed Rust structs locally. Tool execution must never crash the loop; every outcome is converted into a `ToolResult` and fed back into the next LLM turn.

**Tech Stack:** Rust, Tokio, Reqwest, Serde, Serde JSON, Schemars, Anyhow, Thiserror, Async Trait, Tracing, Tracing Subscriber, Tempfile

**Relevant Skills During Execution:** `@superpowers:test-driven-development`, `@superpowers:verification-before-completion`

**Assumptions:**
- Execute this plan in the current repository unless a separate worktree is created first.
- The first real provider is an OpenAI-compatible HTTP API.
- Use environment variables for runtime configuration:
  - `OPENAI_BASE_URL`
  - `OPENAI_API_KEY`
  - `OPENAI_MODEL`

### Task 1: Bootstrap the crate and shared core types

**Files:**
- Create: `Cargo.toml`
- Create: `src/lib.rs`
- Create: `src/main.rs`
- Create: `src/config.rs`
- Create: `src/types.rs`
- Test: `tests/config_defaults.rs`

**Step 1: Write the failing test**

```rust
use exagent::config::AgentConfig;

#[test]
fn agent_config_defaults_are_safe_for_phase1() {
    let cfg = AgentConfig::default();
    assert_eq!(cfg.max_turns, 12);
    assert_eq!(cfg.command_timeout_secs, 30);
    assert_eq!(cfg.max_output_bytes, 8 * 1024);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test config_defaults agent_config_defaults_are_safe_for_phase1 -- --exact`
Expected: FAIL because the crate and `AgentConfig` do not exist yet.

**Step 3: Write the minimal implementation**

`Cargo.toml`

```toml
[package]
name = "exagent"
version = "0.1.0"
edition = "2021"

[dependencies]
anyhow = "1"
async-trait = "0.1"
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
schemars = "0.8"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["fmt", "env-filter"] }

[dev-dependencies]
tempfile = "3"
```

`src/config.rs`

```rust
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,
    pub max_turns: usize,
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub command_timeout_secs: u64,
    pub max_output_bytes: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4.1".to_string()),
            max_turns: 12,
            workspace_root: cwd.clone(),
            cwd,
            command_timeout_secs: 30,
            max_output_bytes: 8 * 1024,
        }
    }
}
```

`src/types.rs`

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Success,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub status: ToolStatus,
    pub content: String,
    pub meta: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantTurn {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}
```

`src/lib.rs`

```rust
pub mod config;
pub mod types;
```

`src/main.rs`

```rust
fn main() {
    println!("exagent bootstrap complete");
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test --test config_defaults agent_config_defaults_are_safe_for_phase1 -- --exact`
Expected: PASS

**Step 5: Commit**

```bash
git add Cargo.toml src/lib.rs src/main.rs src/config.rs src/types.rs tests/config_defaults.rs
git commit -m "chore: bootstrap exagent crate"
```

### Task 2: Add the tool trait and registry

**Files:**
- Create: `src/registry.rs`
- Create: `src/tools/mod.rs`
- Modify: `src/lib.rs`
- Modify: `src/types.rs`
- Test: `tests/registry_dispatch.rs`

**Step 1: Write the failing test**

```rust
use exagent::registry::ToolRegistry;
use exagent::types::ToolCall;
use serde_json::json;

#[tokio::test]
async fn registry_returns_error_result_for_unknown_tool() {
    let registry = ToolRegistry::new();
    let call = ToolCall {
        id: "call_1".into(),
        name: "does_not_exist".into(),
        arguments: json!({}),
    };

    let result = registry.execute(call, None).await;
    assert_eq!(result.tool_name, "does_not_exist");
    assert_eq!(result.status.as_str(), "error");
    assert!(result.content.contains("Unknown tool"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test registry_dispatch registry_returns_error_result_for_unknown_tool -- --exact`
Expected: FAIL because the registry does not exist yet.

**Step 3: Write the minimal implementation**

Add this to `src/types.rs`:

```rust
impl ToolStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
        }
    }
}
```

`src/tools/mod.rs`

```rust
use async_trait::async_trait;
use serde_json::Value;

use crate::registry::ToolContext;
use crate::types::ToolResult;

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> Value;
    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult;
}
```

`src/registry.rs`

```rust
use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use crate::config::AgentConfig;
use crate::tools::Tool;
use crate::types::{ToolCall, ToolResult, ToolStatus};

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub config: AgentConfig,
}

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: HashMap::new() }
    }

    pub fn register<T>(&mut self, tool: T)
    where
        T: Tool + 'static,
    {
        self.tools.insert(tool.name().to_string(), Arc::new(tool));
    }

    pub fn schemas(&self) -> Vec<serde_json::Value> {
        self.tools
            .values()
            .map(|tool| {
                json!({
                    "name": tool.name(),
                    "description": tool.description(),
                    "input_schema": tool.input_schema(),
                })
            })
            .collect()
    }

    pub async fn execute(&self, call: ToolCall, ctx: Option<&ToolContext>) -> ToolResult {
        match (self.tools.get(&call.name), ctx) {
            (Some(tool), Some(ctx)) => tool.execute(call.arguments, ctx).await,
            (Some(_), None) => ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Error,
                content: "Tool context missing".into(),
                meta: None,
            },
            (None, _) => ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Error,
                content: format!("Unknown tool: {}", call.name),
                meta: None,
            },
        }
    }
}
```

Update `src/lib.rs`

```rust
pub mod config;
pub mod registry;
pub mod tools;
pub mod types;
```

**Step 4: Run test to verify it passes**

Run: `cargo test --test registry_dispatch registry_returns_error_result_for_unknown_tool -- --exact`
Expected: PASS

**Step 5: Commit**

```bash
git add src/lib.rs src/registry.rs src/tools/mod.rs src/types.rs tests/registry_dispatch.rs
git commit -m "feat: add tool registry and trait"
```

### Task 3: Implement workspace guards plus `read_file` and `write_file`

**Files:**
- Create: `src/workspace.rs`
- Create: `src/tools/read_file.rs`
- Create: `src/tools/write_file.rs`
- Modify: `src/lib.rs`
- Modify: `src/tools/mod.rs`
- Test: `tests/file_tools.rs`

**Step 1: Write the failing tests**

```rust
use exagent::config::AgentConfig;
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::tools::{read_file::ReadFileTool, write_file::WriteFileTool};
use exagent::types::ToolCall;
use serde_json::json;
use tempfile::tempdir;

#[tokio::test]
async fn read_file_limits_to_requested_range() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "a\nb\nc\nd\n").unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool);

    let ctx = ToolContext {
        config: AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
    };

    let result = registry.execute(
        ToolCall {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: json!({"path": "notes.txt", "start_line": 2, "end_line": 3}),
        },
        Some(&ctx),
    ).await;

    assert_eq!(result.status.as_str(), "success");
    assert!(result.content.contains("b"));
    assert!(result.content.contains("c"));
    assert!(!result.content.contains("a"));
}
```

Add two more tests in the same file:
- `write_file_creates_parent_directories`
- `read_file_rejects_escape_outside_workspace`

**Step 2: Run tests to verify they fail**

Run: `cargo test --test file_tools -- --nocapture`
Expected: FAIL because the workspace guards and file tools do not exist yet.

**Step 3: Write the minimal implementation**

`src/workspace.rs`

```rust
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, Result};

pub fn resolve_workspace_path(root: &Path, raw: &str) -> Result<PathBuf> {
    let candidate = Path::new(raw);
    if candidate.is_absolute() {
        return Err(anyhow!("Absolute paths are not allowed"));
    }

    let mut joined = PathBuf::from(root);
    for component in candidate.components() {
        match component {
            Component::Normal(part) => joined.push(part),
            Component::CurDir => {}
            _ => return Err(anyhow!("Path escapes workspace")),
        }
    }

    Ok(joined)
}
```

`src/tools/read_file.rs`

```rust
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::registry::ToolContext;
use crate::tools::Tool;
use crate::types::{ToolResult, ToolStatus};
use crate::workspace::resolve_workspace_path;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    pub path: String,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
}

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str { "read_file" }
    fn description(&self) -> &'static str { "Read a UTF-8 text file from the workspace" }
    fn input_schema(&self) -> Value { serde_json::to_value(schemars::schema_for!(ReadFileArgs)).unwrap() }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult {
        let parsed: Result<ReadFileArgs, _> = serde_json::from_value(args);
        match parsed.and_then(|args| {
            let path = resolve_workspace_path(&ctx.config.workspace_root, &args.path)
                .map_err(serde_json::Error::custom)?;
            let body = std::fs::read_to_string(&path).map_err(serde_json::Error::custom)?;
            let start = args.start_line.unwrap_or(1);
            let end = args.end_line.unwrap_or(usize::MAX);
            let selected = body
                .lines()
                .enumerate()
                .filter(|(idx, _)| {
                    let line_no = idx + 1;
                    line_no >= start && line_no <= end
                })
                .map(|(_, line)| line)
                .collect::<Vec<_>>()
                .join("\n");
            Ok((path, selected))
        }) {
            Ok((path, content)) => ToolResult {
                tool_call_id: "pending".into(),
                tool_name: self.name().into(),
                status: ToolStatus::Success,
                content,
                meta: Some(json!({ "path": path })),
            },
            Err(err) => ToolResult {
                tool_call_id: "pending".into(),
                tool_name: self.name().into(),
                status: ToolStatus::Error,
                content: err.to_string(),
                meta: None,
            },
        }
    }
}
```

`src/tools/write_file.rs`

```rust
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::registry::ToolContext;
use crate::tools::Tool;
use crate::types::{ToolResult, ToolStatus};
use crate::workspace::resolve_workspace_path;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WriteFileArgs {
    pub path: String,
    pub content: String,
}

pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str { "write_file" }
    fn description(&self) -> &'static str { "Write a UTF-8 text file in the workspace" }
    fn input_schema(&self) -> Value { serde_json::to_value(schemars::schema_for!(WriteFileArgs)).unwrap() }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult {
        let parsed: Result<WriteFileArgs, _> = serde_json::from_value(args);
        match parsed.and_then(|args| {
            let path = resolve_workspace_path(&ctx.config.workspace_root, &args.path)
                .map_err(serde_json::Error::custom)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(serde_json::Error::custom)?;
            }
            std::fs::write(&path, args.content.as_bytes()).map_err(serde_json::Error::custom)?;
            Ok(path)
        }) {
            Ok(path) => ToolResult {
                tool_call_id: "pending".into(),
                tool_name: self.name().into(),
                status: ToolStatus::Success,
                content: format!("Wrote {}", path.display()),
                meta: Some(json!({ "path": path })),
            },
            Err(err) => ToolResult {
                tool_call_id: "pending".into(),
                tool_name: self.name().into(),
                status: ToolStatus::Error,
                content: err.to_string(),
                meta: None,
            },
        }
    }
}
```

Update `src/tools/mod.rs`

```rust
pub mod read_file;
pub mod write_file;
```

Update `src/lib.rs`

```rust
pub mod config;
pub mod registry;
pub mod tools;
pub mod types;
pub mod workspace;
```

Before leaving this task, replace every placeholder `tool_call_id: "pending"` with the actual `ToolCall.id` passed through the registry into the tool execution path.

**Step 4: Run tests to verify they pass**

Run: `cargo test --test file_tools -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/lib.rs src/tools/mod.rs src/tools/read_file.rs src/tools/write_file.rs src/workspace.rs tests/file_tools.rs
git commit -m "feat: add workspace-safe file tools"
```

### Task 4: Implement `run_command` with timeout and output truncation

**Files:**
- Create: `src/tools/run_command.rs`
- Modify: `src/tools/mod.rs`
- Test: `tests/run_command.rs`

**Step 1: Write the failing tests**

```rust
use exagent::config::AgentConfig;
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::tools::run_command::RunCommandTool;
use exagent::types::ToolCall;
use serde_json::json;
use tempfile::tempdir;

#[tokio::test]
async fn run_command_captures_stdout_and_exit_code() {
    let dir = tempdir().unwrap();
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let ctx = ToolContext {
        config: AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
    };

    let result = registry.execute(
        ToolCall {
            id: "call_1".into(),
            name: "run_command".into(),
            arguments: json!({ "command": "printf 'hello'" }),
        },
        Some(&ctx),
    ).await;

    assert_eq!(result.status.as_str(), "success");
    assert_eq!(result.meta.unwrap()["exit_code"], 0);
    assert!(result.content.contains("hello"));
}
```

Add two more tests:
- `run_command_returns_error_status_on_non_zero_exit`
- `run_command_times_out_long_running_process`

**Step 2: Run tests to verify they fail**

Run: `cargo test --test run_command -- --nocapture`
Expected: FAIL because `run_command` does not exist yet.

**Step 3: Write the minimal implementation**

`src/tools/run_command.rs`

```rust
use std::process::Stdio;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::time::{timeout, Duration};

use crate::registry::ToolContext;
use crate::tools::Tool;
use crate::types::{ToolResult, ToolStatus};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunCommandArgs {
    pub command: String,
    pub cwd: Option<String>,
    pub timeout_secs: Option<u64>,
}

pub struct RunCommandTool;

fn truncate(output: &str, max_bytes: usize) -> String {
    if output.len() <= max_bytes {
        output.to_string()
    } else {
        output[..max_bytes].to_string()
    }
}

#[async_trait]
impl Tool for RunCommandTool {
    fn name(&self) -> &'static str { "run_command" }
    fn description(&self) -> &'static str { "Run a shell command inside the workspace" }
    fn input_schema(&self) -> Value { serde_json::to_value(schemars::schema_for!(RunCommandArgs)).unwrap() }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult {
        let parsed: Result<RunCommandArgs, _> = serde_json::from_value(args);
        let args = match parsed {
            Ok(args) => args,
            Err(err) => {
                return ToolResult {
                    tool_call_id: "pending".into(),
                    tool_name: self.name().into(),
                    status: ToolStatus::Error,
                    content: err.to_string(),
                    meta: None,
                };
            }
        };

        let mut command = Command::new("sh");
        command.arg("-lc").arg(&args.command);
        command.current_dir(&ctx.config.cwd);
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let wait = timeout(
            Duration::from_secs(args.timeout_secs.unwrap_or(ctx.config.command_timeout_secs)),
            command.output(),
        ).await;

        match wait {
            Ok(Ok(output)) => {
                let stdout = truncate(&String::from_utf8_lossy(&output.stdout), ctx.config.max_output_bytes);
                let stderr = truncate(&String::from_utf8_lossy(&output.stderr), ctx.config.max_output_bytes);
                let status = if output.status.success() { ToolStatus::Success } else { ToolStatus::Error };
                ToolResult {
                    tool_call_id: "pending".into(),
                    tool_name: self.name().into(),
                    status,
                    content: format!("stdout:\n{}\n\nstderr:\n{}", stdout, stderr),
                    meta: Some(json!({
                        "exit_code": output.status.code(),
                        "stdout": stdout,
                        "stderr": stderr,
                        "timed_out": false,
                    })),
                }
            }
            Ok(Err(err)) => ToolResult {
                tool_call_id: "pending".into(),
                tool_name: self.name().into(),
                status: ToolStatus::Error,
                content: err.to_string(),
                meta: None,
            },
            Err(_) => ToolResult {
                tool_call_id: "pending".into(),
                tool_name: self.name().into(),
                status: ToolStatus::Error,
                content: "Command timed out".into(),
                meta: Some(json!({ "timed_out": true })),
            },
        }
    }
}
```

Update `src/tools/mod.rs`

```rust
pub mod read_file;
pub mod run_command;
pub mod write_file;
```

Before leaving this task, replace every placeholder `tool_call_id: "pending"` here too with the actual `ToolCall.id`.

**Step 4: Run tests to verify they pass**

Run: `cargo test --test run_command -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/tools/mod.rs src/tools/run_command.rs tests/run_command.rs
git commit -m "feat: add command execution tool"
```

### Task 5: Add the LLM interface plus a mock implementation

**Files:**
- Create: `src/llm.rs`
- Modify: `src/lib.rs`
- Modify: `src/types.rs`
- Test: `tests/llm_mock.rs`

**Step 1: Write the failing test**

```rust
use exagent::llm::{LlmClient, MockLlm};
use exagent::types::AssistantTurn;

#[tokio::test]
async fn mock_llm_returns_scripted_turns_in_order() {
    let llm = MockLlm::new(vec![
        AssistantTurn { text: Some("first".into()), tool_calls: vec![] },
        AssistantTurn { text: Some("second".into()), tool_calls: vec![] },
    ]);

    let first = llm.complete(&[], &[]).await.unwrap();
    let second = llm.complete(&[], &[]).await.unwrap();

    assert_eq!(first.text.as_deref(), Some("first"));
    assert_eq!(second.text.as_deref(), Some("second"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test llm_mock mock_llm_returns_scripted_turns_in_order -- --exact`
Expected: FAIL because the LLM interface does not exist yet.

**Step 3: Write the minimal implementation**

Add this to `src/types.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub role: MessageRole,
    pub content: String,
}
```

`src/llm.rs`

```rust
use std::collections::VecDeque;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::types::{AssistantTurn, ConversationMessage};

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        tools: &[serde_json::Value],
    ) -> Result<AssistantTurn>;
}

pub struct MockLlm {
    turns: Mutex<VecDeque<AssistantTurn>>,
}

impl MockLlm {
    pub fn new(turns: Vec<AssistantTurn>) -> Self {
        Self {
            turns: Mutex::new(turns.into()),
        }
    }
}

#[async_trait]
impl LlmClient for MockLlm {
    async fn complete(
        &self,
        _messages: &[ConversationMessage],
        _tools: &[serde_json::Value],
    ) -> Result<AssistantTurn> {
        self.turns
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| anyhow!("MockLlm is out of scripted turns"))
    }
}
```

Update `src/lib.rs`

```rust
pub mod config;
pub mod llm;
pub mod registry;
pub mod tools;
pub mod types;
pub mod workspace;
```

Do not implement the real HTTP client until the mock path is stable. After this task passes, add an `OpenAiCompatibleLlm` in the same file behind the same trait.

**Step 4: Run test to verify it passes**

Run: `cargo test --test llm_mock mock_llm_returns_scripted_turns_in_order -- --exact`
Expected: PASS

**Step 5: Commit**

```bash
git add src/lib.rs src/llm.rs src/types.rs tests/llm_mock.rs
git commit -m "feat: add llm abstraction and mock adapter"
```

### Task 6: Implement the agent loop, transcript logging, and binary wiring

**Files:**
- Create: `src/agent.rs`
- Create: `src/transcript.rs`
- Modify: `src/lib.rs`
- Modify: `src/main.rs`
- Modify: `.gitignore`
- Test: `tests/agent_loop.rs`

**Step 1: Write the failing tests**

```rust
use exagent::agent::Agent;
use exagent::config::AgentConfig;
use exagent::llm::MockLlm;
use exagent::registry::ToolRegistry;
use exagent::tools::write_file::WriteFileTool;
use exagent::types::{AssistantTurn, ToolCall};
use serde_json::json;
use tempfile::tempdir;

#[tokio::test]
async fn agent_runs_until_assistant_returns_no_tool_calls() {
    let dir = tempdir().unwrap();
    let llm = MockLlm::new(vec![
        AssistantTurn {
            text: Some("writing".into()),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "write_file".into(),
                arguments: json!({"path": "out.txt", "content": "hello"}),
            }],
        },
        AssistantTurn {
            text: Some("done".into()),
            tool_calls: vec![],
        },
    ]);

    let mut registry = ToolRegistry::new();
    registry.register(WriteFileTool);

    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let agent = Agent::new(config, Box::new(llm), registry);
    let final_turn = agent.run("create a file").await.unwrap();

    assert_eq!(final_turn.text.as_deref(), Some("done"));
    assert_eq!(std::fs::read_to_string(dir.path().join("out.txt")).unwrap(), "hello");
}
```

Add one more test:
- `agent_feeds_tool_errors_back_into_next_turn`

**Step 2: Run tests to verify they fail**

Run: `cargo test --test agent_loop -- --nocapture`
Expected: FAIL because the agent loop does not exist yet.

**Step 3: Write the minimal implementation**

`src/transcript.rs`

```rust
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

pub fn append_json_line<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    use std::io::Write;
    writeln!(file, "{}", serde_json::to_string(value)?)?;
    Ok(())
}
```

`src/agent.rs`

```rust
use anyhow::{anyhow, Result};

use crate::config::AgentConfig;
use crate::llm::LlmClient;
use crate::registry::{ToolContext, ToolRegistry};
use crate::types::{AssistantTurn, ConversationMessage, MessageRole};

pub struct Agent {
    config: AgentConfig,
    llm: Box<dyn LlmClient>,
    registry: ToolRegistry,
}

impl Agent {
    pub fn new(config: AgentConfig, llm: Box<dyn LlmClient>, registry: ToolRegistry) -> Self {
        Self { config, llm, registry }
    }

    pub async fn run(&self, user_prompt: &str) -> Result<AssistantTurn> {
        let mut messages = vec![ConversationMessage {
            role: MessageRole::User,
            content: user_prompt.to_string(),
        }];

        let ctx = ToolContext {
            config: self.config.clone(),
        };

        let transcript_path = self.config.workspace_root.join(".exagent/transcript.jsonl");
        let mut last_turn = None;

        for _ in 0..self.config.max_turns {
            let turn = self.llm.complete(&messages, &self.registry.schemas()).await?;
            crate::transcript::append_json_line(&transcript_path, &turn)?;

            if let Some(text) = &turn.text {
                messages.push(ConversationMessage {
                    role: MessageRole::Assistant,
                    content: text.clone(),
                });
            }

            if turn.tool_calls.is_empty() {
                last_turn = Some(turn);
                break;
            }

            for call in turn.tool_calls.clone() {
                let result = self.registry.execute(call.clone(), Some(&ctx)).await;
                crate::transcript::append_json_line(&transcript_path, &result)?;
                messages.push(ConversationMessage {
                    role: MessageRole::Tool,
                    content: serde_json::to_string(&result)?,
                });
            }

            last_turn = Some(turn);
        }

        last_turn.ok_or_else(|| anyhow!("Agent exited without producing a turn"))
    }
}
```

Update `src/main.rs`

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let prompt = std::env::args().nth(1).expect("usage: cargo run -- '<prompt>'");
    let config = exagent::config::AgentConfig::default();

    let mut registry = exagent::registry::ToolRegistry::new();
    registry.register(exagent::tools::read_file::ReadFileTool);
    registry.register(exagent::tools::write_file::WriteFileTool);
    registry.register(exagent::tools::run_command::RunCommandTool);

    let llm = exagent::llm::MockLlm::new(vec![
        exagent::types::AssistantTurn {
            text: Some("No real LLM configured yet".into()),
            tool_calls: vec![],
        }
    ]);

    let agent = exagent::agent::Agent::new(config, Box::new(llm), registry);
    let final_turn = agent.run(&prompt).await?;
    println!("{}", final_turn.text.unwrap_or_default());
    Ok(())
}
```

Update `.gitignore`

```gitignore
/target
/.exagent
```

Before leaving this task:
- Replace the `MockLlm` in `main.rs` with the real `OpenAiCompatibleLlm` once Task 5's HTTP client is ready.
- Add a hard failure when the loop reaches `max_turns` without a final assistant turn.

**Step 4: Run tests to verify they pass**

Run: `cargo test --test agent_loop -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add .gitignore src/agent.rs src/lib.rs src/main.rs src/transcript.rs tests/agent_loop.rs
git commit -m "feat: add phase1 agent loop"
```

### Task 7: Add the real OpenAI-compatible adapter and run end-to-end verification

**Files:**
- Modify: `src/llm.rs`
- Modify: `src/main.rs`
- Test: `tests/llm_http.rs`

**Step 1: Write the failing test**

```rust
use exagent::llm::OpenAiCompatibleLlm;

#[test]
fn openai_client_requires_model_configuration() {
    let build = OpenAiCompatibleLlm::from_env();
    assert!(build.is_err());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test llm_http openai_client_requires_model_configuration -- --exact`
Expected: FAIL because the HTTP adapter does not exist yet.

**Step 3: Write the minimal implementation**

Extend `src/llm.rs` with:

```rust
pub struct OpenAiCompatibleLlm {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl OpenAiCompatibleLlm {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            client: reqwest::Client::new(),
            base_url: std::env::var("OPENAI_BASE_URL")?,
            api_key: std::env::var("OPENAI_API_KEY")?,
            model: std::env::var("OPENAI_MODEL")?,
        })
    }
}
```

Then implement `LlmClient` by:
- POSTing `messages` and `tools` to the compatible endpoint
- Parsing assistant text plus tool calls
- Converting provider-specific JSON into `AssistantTurn`

Update `src/main.rs` to:
- Build `OpenAiCompatibleLlm::from_env()`
- Fail fast with a clear error if the environment is incomplete
- Keep `MockLlm` available only in tests

**Step 4: Run tests to verify they pass**

Run: `cargo test --test llm_http openai_client_requires_model_configuration -- --exact`
Expected: PASS

**Step 5: Run end-to-end verification**

Run: `cargo test`
Expected: PASS

Run: `cargo run -- "Write a file named hello.txt with the text hi"`
Expected: The binary starts, calls the configured model, and either writes the file or returns a structured tool error without crashing.

Run: `cat .exagent/transcript.jsonl`
Expected: JSONL records for assistant turns and tool results.

**Step 6: Commit**

```bash
git add src/llm.rs src/main.rs tests/llm_http.rs
git commit -m "feat: add openai-compatible llm adapter"
```

## Final Verification Checklist

- `cargo test`
- `cargo fmt -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo run -- "Read the file Cargo.toml"`

## Notes for the Implementer

- Keep the runtime provider-neutral. Do not let OpenAI-specific response types leak into `src/agent.rs`.
- Do not panic inside tools. Return structured `ToolResult` errors instead.
- Make sure the registry passes the original `ToolCall.id` into each tool result.
- Enforce workspace path boundaries before any file read or write.
- Truncate command output before storing it in message history.
- Stop the loop both when there are no tool calls and when `max_turns` is reached.
