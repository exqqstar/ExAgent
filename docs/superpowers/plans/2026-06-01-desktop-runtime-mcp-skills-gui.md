# Desktop Runtime Controls and MCP/Skills GUI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the next ExAgent Desktop GUI slice: real chat-time model/thinking controls plus Settings pages for MCP server and skill configuration.

**Architecture:** Keep the runtime boundary honest: Composer controls send real per-turn model/thinking overrides through `TurnStartParams`, while Settings persists MCP/Skills configuration as desktop runtime settings that can be displayed and consumed by future registry work. The first MCP/Skills GUI slice supports editing, validation, persistence, and visibility; it does not claim MCP processes are connected unless runtime support exists.

**Tech Stack:** Rust, serde, Tauri v2 commands, React, TypeScript, Vite, Zustand, shadcn/ui source components, Radix primitives, lucide-react, Vitest, React Testing Library, Cargo tests.

---

## Spec Criteria

This plan intentionally includes only the first useful GUI slice.

- Composer must expose a model selector and thinking mode control before sending a turn.
- Composer controls must affect `turn_start`; they cannot be cosmetic-only controls.
- Model selection is scoped to the active provider. The UI may use discovered models or manual model text, but it must not switch provider credentials from the chat box.
- Thinking mode supports `auto`, `low`, `medium`, and `high`, matching `src/config.rs::ThinkingMode`.
- Settings gains tabs for `Providers`, `Runtime`, `MCP`, and `Skills`. No Diagnostics/Safety tab is included in this slice.
- Runtime settings include default model, default thinking mode, and saved runtime presets so repeated workflows do not require reselecting controls every turn.
- MCP settings support adding, editing, enabling, disabling, and removing server entries with `name`, `command`, `args`, `env`, and `working_directory`.
- Skills settings support adding, editing, enabling, disabling, and removing skill roots with `name`, `path`, and `scope`.
- MCP/Skills settings must be persisted to the desktop app settings file and reloaded after app restart.
- Inspector must show current effective model, thinking mode, configured MCP server count, and enabled skill count for the active workbench.
- The UI must label MCP/Skills state as configured unless a runtime activation path is implemented in the same task.
- Desktop app styling must keep the current product direction from `DESIGN.md`: dense, quiet, native, no gradients, no hero layout, no nested cards.

## Non-Goals

- No Diagnostics/Safety page in this slice.
- No MCP process supervisor or protocol client in this slice.
- No skill execution engine or prompt-injection runtime in this slice.
- No provider switching from the chat composer.
- No full JSON editor as the primary configuration UI.

## Overall Acceptance Criteria

- `cargo fmt --check` passes.
- `cargo test` passes.
- `git diff --check` passes.
- `npm test --prefix apps/desktop` passes.
- `npm run build --prefix apps/desktop` passes.
- The desktop mock runtime shows Composer controls for model and thinking mode.
- In Tauri runtime, sending a prompt passes selected model and thinking mode through `turn_start`.
- Rust tests prove per-turn model override reaches `TurnContextItem` and does not mutate the next turn.
- Settings dialog has accessible tabs for Providers, Runtime, MCP, and Skills.
- Runtime, MCP, and Skills settings persist through Tauri commands and reload from disk.
- Inspector reflects effective runtime controls and configured MCP/Skills counts.

## File Structure

Root crate:

- Modify `src/config.rs`: expose model override support through existing `AgentConfig`.
- Modify `src/app_server/protocol.rs`: add `model` to `TurnContextOverrides`.
- Modify `src/runtime/thread_runtime.rs`: add `model` to `ThreadTurnContext`.
- Modify `src/app_server/thread_manager.rs`: resolve per-turn model with existing turn context.
- Modify `src/runtime/thread_session/turn.rs`: apply per-turn model only to the current turn.
- Modify `src/runtime/context.rs`: include selected model in persisted turn context.
- Modify `src/state/session.rs`: persist optional per-turn model with serde defaults.
- Modify `tests/api_server.rs`: assert `turn_context.model` deserializes.
- Modify `tests/app_server_boundary.rs`: assert model override reaches persisted context and does not leak.

Desktop Tauri:

- Modify `apps/desktop/src-tauri/src/settings.rs`: add runtime/MCP/Skills settings DTOs, persistence, and validation.
- Modify `apps/desktop/src-tauri/src/commands.rs`: add `runtime_settings_get` and `runtime_settings_save`; extend `turn_start`.
- Modify `apps/desktop/src-tauri/src/lib.rs`: register new commands.
- Test `apps/desktop/src-tauri/tests/runtime_settings.rs`: persistence and validation.

Desktop React:

- Modify `apps/desktop/src/types.ts`: add runtime settings, MCP server, skill root, and turn override types.
- Modify `apps/desktop/src/api/exagentClient.ts`: add runtime settings client calls and extend `startTurn`.
- Modify `apps/desktop/src/stores/workbenchStore.ts`: hold runtime settings and selected composer overrides.
- Modify `apps/desktop/src/components/Composer.tsx`: add model selector, thinking mode control, preset selector.
- Modify `apps/desktop/src/components/SettingsDialog.tsx`: add tab state and Runtime/MCP/Skills panels.
- Create `apps/desktop/src/components/settings/RuntimeSettingsPanel.tsx`: runtime defaults and presets.
- Create `apps/desktop/src/components/settings/McpSettingsPanel.tsx`: MCP server editor.
- Create `apps/desktop/src/components/settings/SkillsSettingsPanel.tsx`: skill root editor.
- Modify `apps/desktop/src/components/Inspector.tsx`: show runtime controls and configuration counts.
- Modify `apps/desktop/src/App.test.tsx`: assert UI surfaces and persistence behavior.

## Task 1: Runtime Boundary Supports Per-Turn Model

**Files:**
- Modify: `src/app_server/protocol.rs`
- Modify: `src/runtime/thread_runtime.rs`
- Modify: `src/app_server/thread_manager.rs`
- Modify: `src/runtime/thread_session/turn.rs`
- Modify: `src/runtime/context.rs`
- Modify: `src/state/session.rs`
- Test: `tests/api_server.rs`
- Test: `tests/app_server_boundary.rs`

- [ ] **Step 1: Add failing API deserialization assertion**

In `tests/api_server.rs`, extend the existing `turn_start_route_accepts_thread_id_and_prompt` request body:

```rust
json!({
    "thread_id": "session_123",
    "prompt": "continue phase2",
    "workspace_root": ".",
    "turn_context": {
        "model": "gpt-4.1-mini",
        "thinking_mode": "high"
    }
})
```

Inside the test boundary's `turn_start`, add:

```rust
assert_eq!(
    params
        .turn_context
        .as_ref()
        .and_then(|context| context.model.as_deref()),
    Some("gpt-4.1-mini")
);
```

- [ ] **Step 2: Run the failing API test**

Run:

```bash
cargo test --test api_server turn_start_route_accepts_thread_id_and_prompt
```

Expected: compile failure because `TurnContextOverrides::model` does not exist.

- [ ] **Step 3: Add model to protocol DTO**

In `src/app_server/protocol.rs`, change `TurnContextOverrides` to:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TurnContextOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_mode: Option<ThinkingMode>,
}
```

Run:

```bash
rg -n "TurnContextOverrides \\{" src tests
```

For every literal, add `model: None` unless the test is explicitly exercising a model override.

- [ ] **Step 4: Carry model in runtime turn context**

In `src/runtime/thread_runtime.rs`, update `ThreadTurnContext`:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadTurnContext {
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
    pub thinking_mode: Option<ThinkingMode>,
}
```

In `src/app_server/thread_manager.rs`, update `resolve_turn_context`:

```rust
let model = overrides
    .model
    .as_deref()
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .map(str::to_string);
let thinking_mode = overrides.thinking_mode;
let resolved_snapshot = OverridePolicy::apply_turn_context(snapshot, overrides)?;
Ok(Some(ThreadTurnContext {
    cwd: Some(resolved_snapshot.cwd),
    model,
    thinking_mode,
}))
```

- [ ] **Step 5: Apply model only to current turn**

In `src/runtime/thread_session/turn.rs`, update helper signature:

```rust
fn config_for_turn(
    config: &AgentConfig,
    turn_model: Option<&str>,
    turn_thinking_mode: Option<ThinkingMode>,
) -> AgentConfig {
    let mut config = config.clone();
    if let Some(model) = turn_model.map(str::trim).filter(|value| !value.is_empty()) {
        config.model = model.to_string();
    }
    if let Some(thinking_mode) = turn_thinking_mode {
        config.thinking_mode = Some(thinking_mode);
    }
    config
}
```

Update call sites to pass `turn_context.model.as_deref()` where a `ThreadTurnContext` exists. Existing calls without context pass `None`.

- [ ] **Step 6: Persist selected model in turn context**

In `src/state/session.rs`, extend `TurnContextItem` with a serde-default optional field:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub model: Option<String>,
```

In `src/runtime/context.rs`, ensure `PromptContext::for_turn` writes the effective model:

```rust
model: config.model.clone(),
```

If `TurnContextItem` already has `model`, this step is complete when tests prove the override value is the per-turn value.

- [ ] **Step 7: Add boundary regression test**

In `tests/app_server_boundary.rs`, add:

```rust
#[tokio::test]
async fn turn_context_model_reaches_turn_context_without_mutating_later_turns() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().to_path_buf();
    let config = AgentConfig {
        model: "base-model".to_string(),
        workspace_root: workspace.clone(),
        cwd: workspace.clone(),
        ..AgentConfig::default()
    };
    let boundary = AppServerService::with_llm(
        config,
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: Some("first done".into()),
                tool_calls: vec![],
            },
            AssistantTurn {
                text: Some("second done".into()),
                tool_calls: vec![],
            },
        ])),
        ToolRegistry::new,
    );
    let started = boundary
        .thread_start(ThreadStartParams {
            workspace_root: Some(workspace.display().to_string()),
            cwd: None,
        })
        .unwrap();

    boundary
        .turn_start(TurnStartParams {
            thread_id: started.thread.id.clone(),
            prompt: "first".into(),
            workspace_root: Some(workspace.display().to_string()),
            turn_context: Some(TurnContextOverrides {
                cwd: None,
                model: Some("override-model".into()),
                thinking_mode: None,
            }),
        })
        .await
        .unwrap();

    boundary
        .turn_start(TurnStartParams {
            thread_id: started.thread.id.clone(),
            prompt: "second".into(),
            workspace_root: Some(workspace.display().to_string()),
            turn_context: None,
        })
        .await
        .unwrap();

    let rollout_paths = exagent::state::rollout::rollout_paths(&workspace, &started.thread.id);
    let contexts: Vec<_> = exagent::state::rollout::RolloutStore::read_items_blocking(
        &rollout_paths.rollout_path,
    )
        .unwrap()
        .into_iter()
        .filter_map(|item| match item {
            exagent::state::rollout::RolloutItem::TurnContext(context) => Some(context.model),
            _ => None,
        })
        .collect();

    assert!(contexts.iter().any(|model| model.as_deref() == Some("override-model")));
    assert!(contexts.iter().any(|model| model.as_deref() == Some("base-model")));
}
```

- [ ] **Step 8: Verify Task 1**

Run:

```bash
cargo test --test api_server turn_start_route_accepts_thread_id_and_prompt
cargo test --test app_server_boundary turn_context_model_reaches_turn_context_without_mutating_later_turns
```

Expected: both tests pass.

- [ ] **Step 9: Commit**

```bash
git add src/app_server/protocol.rs src/runtime/thread_runtime.rs src/app_server/thread_manager.rs src/runtime/thread_session/turn.rs src/runtime/context.rs src/state/session.rs tests/api_server.rs tests/app_server_boundary.rs
git commit -m "feat: support per-turn model overrides"
```

## Task 2: Desktop Runtime Settings DTOs and Persistence

**Files:**
- Modify: `apps/desktop/src-tauri/src/settings.rs`
- Create: `apps/desktop/src-tauri/tests/runtime_settings.rs`

- [ ] **Step 1: Add failing runtime settings persistence tests**

Create `apps/desktop/src-tauri/tests/runtime_settings.rs`:

```rust
use std::sync::{Arc, Mutex};

use exagent_desktop::settings::{
    DesktopSettingsStore, McpServerSettings, RuntimePresetSettings, RuntimeSettingsSaveRequest,
    SkillRootSettings,
};

#[derive(Default)]
struct MemorySecretStore {
    values: Mutex<std::collections::HashMap<String, String>>,
}

impl exagent_desktop::settings::SecretStore for MemorySecretStore {
    fn get_secret(&self, account: &str) -> anyhow::Result<Option<String>> {
        Ok(self.values.lock().unwrap().get(account).cloned())
    }

    fn set_secret(&self, account: &str, secret: &str) -> anyhow::Result<()> {
        self.values.lock().unwrap().insert(account.into(), secret.into());
        Ok(())
    }

    fn delete_secret(&self, account: &str) -> anyhow::Result<()> {
        self.values.lock().unwrap().remove(account);
        Ok(())
    }
}

#[tokio::test]
async fn runtime_settings_round_trip_defaults_mcp_and_skills() {
    let temp = tempfile::tempdir().unwrap();
    let store = DesktopSettingsStore::with_secret_store(
        temp.path().join("settings.json"),
        Arc::new(MemorySecretStore::default()),
    );

    store
        .save_runtime_settings(RuntimeSettingsSaveRequest {
            default_model: "gpt-4.1-mini".into(),
            default_thinking_mode: Some(exagent::config::ThinkingMode::High),
            presets: vec![RuntimePresetSettings {
                id: "deep-code".into(),
                name: "Deep Code".into(),
                model: "gpt-4.1".into(),
                thinking_mode: Some(exagent::config::ThinkingMode::High),
            }],
            mcp_servers: vec![McpServerSettings {
                id: "filesystem".into(),
                name: "Filesystem".into(),
                enabled: true,
                command: "npx".into(),
                args: vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into()],
                env: vec![("ROOT".into(), "/tmp".into())],
                working_directory: None,
            }],
            skill_roots: vec![SkillRootSettings {
                id: "local-skills".into(),
                name: "Local skills".into(),
                enabled: true,
                path: temp.path().display().to_string(),
                scope: "global".into(),
            }],
        })
        .await
        .unwrap();

    let loaded = store.load_runtime_settings().await.unwrap();
    assert_eq!(loaded.default_model, "gpt-4.1-mini");
    assert_eq!(loaded.default_thinking_mode, Some(exagent::config::ThinkingMode::High));
    assert_eq!(loaded.presets[0].id, "deep-code");
    assert_eq!(loaded.mcp_servers[0].command, "npx");
    assert_eq!(loaded.skill_roots[0].name, "Local skills");
}

#[tokio::test]
async fn runtime_settings_reject_blank_mcp_command() {
    let temp = tempfile::tempdir().unwrap();
    let store = DesktopSettingsStore::with_secret_store(
        temp.path().join("settings.json"),
        Arc::new(MemorySecretStore::default()),
    );

    let error = store
        .save_runtime_settings(RuntimeSettingsSaveRequest {
            default_model: "gpt-4.1".into(),
            default_thinking_mode: None,
            presets: vec![],
            mcp_servers: vec![McpServerSettings {
                id: "bad".into(),
                name: "Bad".into(),
                enabled: true,
                command: " ".into(),
                args: vec![],
                env: vec![],
                working_directory: None,
            }],
            skill_roots: vec![],
        })
        .await
        .unwrap_err();

    assert!(error.to_string().contains("MCP command is required"));
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p exagent-desktop --test runtime_settings
```

Expected: compile failure because runtime settings DTOs and methods do not exist.

- [ ] **Step 3: Add DTOs**

In `apps/desktop/src-tauri/src/settings.rs`, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimePresetSettings {
    pub id: String,
    pub name: String,
    pub model: String,
    pub thinking_mode: Option<exagent::config::ThinkingMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerSettings {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillRootSettings {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub path: String,
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSettingsResponse {
    pub default_model: String,
    pub default_thinking_mode: Option<exagent::config::ThinkingMode>,
    pub presets: Vec<RuntimePresetSettings>,
    pub mcp_servers: Vec<McpServerSettings>,
    pub skill_roots: Vec<SkillRootSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSettingsSaveRequest {
    pub default_model: String,
    pub default_thinking_mode: Option<exagent::config::ThinkingMode>,
    pub presets: Vec<RuntimePresetSettings>,
    pub mcp_servers: Vec<McpServerSettings>,
    pub skill_roots: Vec<SkillRootSettings>,
}
```

- [ ] **Step 4: Extend settings file**

Add these fields to `SettingsFile`:

```rust
#[serde(default = "default_runtime_model")]
runtime_default_model: String,
#[serde(default)]
runtime_default_thinking_mode: Option<exagent::config::ThinkingMode>,
#[serde(default)]
runtime_presets: Vec<RuntimePresetSettings>,
#[serde(default)]
mcp_servers: Vec<McpServerSettings>,
#[serde(default)]
skill_roots: Vec<SkillRootSettings>,
```

Add helper:

```rust
fn default_runtime_model() -> String {
    SettingsFile::default().model
}
```

Ensure `Default for SettingsFile` initializes the new fields:

```rust
runtime_default_model: profile.default_model.to_string(),
runtime_default_thinking_mode: None,
runtime_presets: Vec::new(),
mcp_servers: Vec::new(),
skill_roots: Vec::new(),
```

- [ ] **Step 5: Add load/save methods and validation**

Add methods on `DesktopSettingsStore`:

```rust
pub async fn load_runtime_settings(&self) -> Result<RuntimeSettingsResponse> {
    let file = self.load_file().await?;
    Ok(RuntimeSettingsResponse {
        default_model: normalized_or_default(&file.runtime_default_model, &file.model),
        default_thinking_mode: file.runtime_default_thinking_mode,
        presets: file.runtime_presets,
        mcp_servers: file.mcp_servers,
        skill_roots: file.skill_roots,
    })
}

pub async fn save_runtime_settings(
    &self,
    request: RuntimeSettingsSaveRequest,
) -> Result<RuntimeSettingsResponse> {
    validate_runtime_settings(&request)?;
    let mut file = self.load_file().await?;
    file.runtime_default_model = normalized_or_default(&request.default_model, &file.model);
    file.runtime_default_thinking_mode = request.default_thinking_mode;
    file.runtime_presets = request.presets;
    file.mcp_servers = request.mcp_servers;
    file.skill_roots = request.skill_roots;
    self.save_file(&file).await?;
    self.load_runtime_settings().await
}
```

Add validation:

```rust
fn validate_runtime_settings(request: &RuntimeSettingsSaveRequest) -> Result<()> {
    for server in &request.mcp_servers {
        if server.name.trim().is_empty() {
            anyhow::bail!("MCP server name is required");
        }
        if server.command.trim().is_empty() {
            anyhow::bail!("MCP command is required");
        }
        for (key, _) in &server.env {
            if key.trim().is_empty() {
                anyhow::bail!("MCP environment variable name is required");
            }
        }
    }
    for root in &request.skill_roots {
        if root.name.trim().is_empty() {
            anyhow::bail!("Skill root name is required");
        }
        if root.enabled && root.path.trim().is_empty() {
            anyhow::bail!("Enabled skill root path is required");
        }
    }
    Ok(())
}
```

- [ ] **Step 6: Preserve runtime settings when provider settings are saved**

In `save_provider_settings`, load the existing file before creating the new provider file:

```rust
let existing = self.load_file().await.unwrap_or_default();
let file = SettingsFile {
    provider_id: provider.id.clone(),
    base_url: normalized_or_default(&request.base_url, &provider.default_base_url),
    model: normalized_or_default(&request.model, &provider.default_model),
    last_connection_status: None,
    last_connection_message: None,
    last_connection_checked_at: None,
    runtime_default_model: existing.runtime_default_model,
    runtime_default_thinking_mode: existing.runtime_default_thinking_mode,
    runtime_presets: existing.runtime_presets,
    mcp_servers: existing.mcp_servers,
    skill_roots: existing.skill_roots,
};
```

This prevents connecting a provider from erasing MCP/Skills settings.

- [ ] **Step 7: Verify Task 2**

Run:

```bash
cargo test -p exagent-desktop --test runtime_settings
```

Expected: tests pass.

- [ ] **Step 8: Commit**

```bash
git add apps/desktop/src-tauri/src/settings.rs apps/desktop/src-tauri/tests/runtime_settings.rs
git commit -m "feat: persist desktop runtime settings"
```

## Task 3: Tauri Commands and Client Types

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Modify: `apps/desktop/src/types.ts`
- Modify: `apps/desktop/src/api/exagentClient.ts`

- [ ] **Step 1: Add command imports**

In `apps/desktop/src-tauri/src/commands.rs`, extend settings imports:

```rust
RuntimeSettingsResponse, RuntimeSettingsSaveRequest,
```

- [ ] **Step 2: Add runtime settings commands**

In `apps/desktop/src-tauri/src/commands.rs`, add:

```rust
#[tauri::command]
pub async fn runtime_settings_get(
    state: State<'_, DesktopState>,
) -> CommandResult<RuntimeSettingsResponse> {
    state
        .settings
        .load_runtime_settings()
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn runtime_settings_save(
    state: State<'_, DesktopState>,
    request: RuntimeSettingsSaveRequest,
) -> CommandResult<RuntimeSettingsResponse> {
    state
        .settings
        .save_runtime_settings(request)
        .await
        .map_err(error_string)
}
```

- [ ] **Step 3: Extend Tauri `turn_start` command**

Change command signature:

```rust
pub async fn turn_start(
    state: State<'_, DesktopState>,
    project_id: String,
    thread_id: String,
    prompt: String,
    model: Option<String>,
    thinking_mode: Option<exagent::config::ThinkingMode>,
) -> CommandResult<TurnStartResponse> {
```

Build turn context:

```rust
let turn_context = match (
    model.as_deref().map(str::trim).filter(|value| !value.is_empty()),
    thinking_mode,
) {
    (None, None) => None,
    (model, thinking_mode) => Some(exagent::app_server::protocol::TurnContextOverrides {
        cwd: None,
        model: model.map(str::to_string),
        thinking_mode,
    }),
};
```

Pass it into `TurnStartParams { turn_context, ... }`.

- [ ] **Step 4: Register commands**

In `apps/desktop/src-tauri/src/lib.rs`, add both commands to `tauri::generate_handler!`:

```rust
commands::runtime_settings_get,
commands::runtime_settings_save,
```

- [ ] **Step 5: Add TypeScript types**

In `apps/desktop/src/types.ts`, add:

```ts
export type ThinkingMode = "auto" | "low" | "medium" | "high";

export interface RuntimePresetSettings {
  id: string;
  name: string;
  model: string;
  thinking_mode: ThinkingMode | null;
}

export interface McpServerSettings {
  id: string;
  name: string;
  enabled: boolean;
  command: string;
  args: string[];
  env: [string, string][];
  working_directory: string | null;
}

export interface SkillRootSettings {
  id: string;
  name: string;
  enabled: boolean;
  path: string;
  scope: string;
}

export interface RuntimeSettingsResponse {
  default_model: string;
  default_thinking_mode: ThinkingMode | null;
  presets: RuntimePresetSettings[];
  mcp_servers: McpServerSettings[];
  skill_roots: SkillRootSettings[];
}

export type RuntimeSettingsSaveRequest = RuntimeSettingsResponse;
```

- [ ] **Step 6: Add client methods and extend startTurn**

In `apps/desktop/src/api/exagentClient.ts`, update imports with the new types, then change `startTurn`:

```ts
export async function startTurn(
  projectId: string,
  threadId: string,
  prompt: string,
  model?: string | null,
  thinkingMode?: ThinkingMode | null
): Promise<TurnStartResponse> {
  return invokeCommand<TurnStartResponse>("turn_start", {
    projectId,
    threadId,
    prompt,
    model: model?.trim() ? model.trim() : null,
    thinkingMode: thinkingMode ?? null
  });
}
```

Add runtime settings functions:

```ts
export async function getRuntimeSettings(): Promise<RuntimeSettingsResponse> {
  if (!isTauriRuntime()) {
    return {
      default_model: mockProviderSettings.config.model,
      default_thinking_mode: "auto",
      presets: [
        { id: "fast", name: "Fast", model: "gpt-4.1-mini", thinking_mode: "low" },
        { id: "deep", name: "Deep", model: "gpt-4.1", thinking_mode: "high" }
      ],
      mcp_servers: [],
      skill_roots: []
    };
  }
  return invokeCommand<RuntimeSettingsResponse>("runtime_settings_get");
}

export async function saveRuntimeSettings(
  request: RuntimeSettingsSaveRequest
): Promise<RuntimeSettingsResponse> {
  if (!isTauriRuntime()) {
    return request;
  }
  return invokeCommand<RuntimeSettingsResponse>("runtime_settings_save", { request });
}
```

Export both through `exagentClient`.

- [ ] **Step 7: Verify Task 3**

Run:

```bash
npm test --prefix apps/desktop -- --run
cargo test -p exagent-desktop
```

Expected: TypeScript compiles through Vitest and Tauri crate tests pass.

- [ ] **Step 8: Commit**

```bash
git add apps/desktop/src-tauri/src/commands.rs apps/desktop/src-tauri/src/lib.rs apps/desktop/src/types.ts apps/desktop/src/api/exagentClient.ts
git commit -m "feat: expose runtime settings to desktop UI"
```

## Task 4: Workbench Store Runtime State

**Files:**
- Modify: `apps/desktop/src/stores/workbenchStore.ts`
- Modify: `apps/desktop/src/types.ts`

- [ ] **Step 1: Add state fields**

In `WorkbenchSnapshot` in `apps/desktop/src/types.ts`, add:

```ts
runtimeSettings: RuntimeSettingsResponse | null;
selectedModel: string | null;
selectedThinkingMode: ThinkingMode | null;
```

Update all `WorkbenchSnapshot` literals to include:

```ts
runtimeSettings: null,
selectedModel: null,
selectedThinkingMode: null,
```

- [ ] **Step 2: Add store actions**

In `WorkbenchState`, add:

```ts
setSelectedModel: (model: string | null) => void;
setSelectedThinkingMode: (thinkingMode: ThinkingMode | null) => void;
applyRuntimePreset: (presetId: string) => void;
saveRuntimeSettings: (settings: RuntimeSettingsSaveRequest) => Promise<void>;
```

- [ ] **Step 3: Load runtime settings with workbench snapshot**

Inside `loadWorkbench`, after `getWorkbenchSnapshot()`:

```ts
const runtimeSettings = await exagentClient.getRuntimeSettings();
set({
  ...snapshot,
  runtimeSettings,
  selectedModel: runtimeSettings.default_model,
  selectedThinkingMode: runtimeSettings.default_thinking_mode,
  loading: false,
  error: null
});
```

- [ ] **Step 4: Send overrides from store**

In `sendPrompt`, change:

```ts
await exagentClient.startTurn(projectId, threadId, prompt);
```

to:

```ts
await exagentClient.startTurn(
  projectId,
  threadId,
  prompt,
  get().selectedModel,
  get().selectedThinkingMode
);
```

- [ ] **Step 5: Implement preset actions**

Add implementations:

```ts
setSelectedModel(model) {
  set({ selectedModel: model?.trim() ? model.trim() : null });
},

setSelectedThinkingMode(selectedThinkingMode) {
  set({ selectedThinkingMode });
},

applyRuntimePreset(presetId) {
  const preset = get().runtimeSettings?.presets.find((item) => item.id === presetId);
  if (!preset) {
    return;
  }
  set({
    selectedModel: preset.model,
    selectedThinkingMode: preset.thinking_mode
  });
},

async saveRuntimeSettings(settings) {
  try {
    const runtimeSettings = await exagentClient.saveRuntimeSettings(settings);
    set({
      runtimeSettings,
      selectedModel: runtimeSettings.default_model,
      selectedThinkingMode: runtimeSettings.default_thinking_mode
    });
  } catch (error) {
    set({ error: errorMessage(error) });
  }
}
```

- [ ] **Step 6: Verify Task 4**

Run:

```bash
npm test --prefix apps/desktop -- --run
```

Expected: existing tests pass after snapshot literals are updated.

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src/types.ts apps/desktop/src/stores/workbenchStore.ts
git commit -m "feat: track runtime controls in workbench state"
```

## Task 5: Composer Runtime Controls

**Files:**
- Modify: `apps/desktop/src/components/Composer.tsx`
- Modify: `apps/desktop/src/App.test.tsx`

- [ ] **Step 1: Add failing UI test**

In `apps/desktop/src/App.test.tsx`, add:

```ts
it("shows model and thinking controls in the composer", async () => {
  render(<App />);

  await screen.findByText("Session restored");

  expect(screen.getByLabelText("Composer model")).toHaveValue("gpt-4.1");
  expect(screen.getByRole("button", { name: "Thinking auto" })).toHaveAttribute("aria-pressed", "true");
  expect(screen.getByRole("button", { name: "Thinking high" })).toBeInTheDocument();
});
```

- [ ] **Step 2: Run failing test**

Run:

```bash
npm test --prefix apps/desktop -- --run App.test.tsx
```

Expected: failure because controls do not exist.

- [ ] **Step 3: Add controls to Composer**

In `apps/desktop/src/components/Composer.tsx`, import:

```ts
import { Brain, ChevronDown } from "lucide-react";
import type { ThinkingMode } from "@/types";
import {
  applyRuntimePreset,
  setSelectedModel,
  setSelectedThinkingMode
} from "@/stores/workbenchStore";
```

Add mode list:

```ts
const thinkingModes: Array<{ value: ThinkingMode; label: string }> = [
  { value: "auto", label: "Auto" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" }
];
```

Above the textarea, add:

```tsx
<div className="mb-2 flex flex-wrap items-center gap-2">
  <label className="flex min-w-[180px] flex-1 items-center gap-2 rounded-md border border-border bg-surface-2 px-2 py-1.5 text-xs text-muted">
    Model
    <input
      aria-label="Composer model"
      className="min-w-0 flex-1 bg-transparent font-mono text-xs text-ink outline-none"
      value={state.selectedModel ?? state.runtimeSettings?.default_model ?? ""}
      onChange={(event) => setSelectedModel(event.target.value)}
    />
  </label>

  <div className="flex items-center gap-1" aria-label="Thinking mode">
    <Brain className="h-4 w-4 text-subtle" />
    {thinkingModes.map((mode) => (
      <Button
        key={mode.value}
        type="button"
        variant={state.selectedThinkingMode === mode.value ? "secondary" : "ghost"}
        size="sm"
        aria-label={`Thinking ${mode.value}`}
        aria-pressed={state.selectedThinkingMode === mode.value}
        onClick={() => setSelectedThinkingMode(mode.value)}
      >
        {mode.label}
      </Button>
    ))}
  </div>

  {state.runtimeSettings?.presets.length ? (
    <select
      aria-label="Runtime preset"
      className="h-8 rounded-md border border-border bg-surface-2 px-2 text-xs text-ink"
      defaultValue=""
      onChange={(event) => {
        if (event.target.value) {
          applyRuntimePreset(event.target.value);
        }
      }}
    >
      <option value="">Preset</option>
      {state.runtimeSettings.presets.map((preset) => (
        <option key={preset.id} value={preset.id}>
          {preset.name}
        </option>
      ))}
    </select>
  ) : null}
</div>
```

Use existing button/input styles if class names need minor adjustment, but keep labels and ARIA names intact for tests.

- [ ] **Step 4: Export store helper functions**

At the bottom of `workbenchStore.ts`, export:

```ts
export const setSelectedModel = (model: string | null) =>
  useWorkbenchStore.getState().setSelectedModel(model);
export const setSelectedThinkingMode = (thinkingMode: ThinkingMode | null) =>
  useWorkbenchStore.getState().setSelectedThinkingMode(thinkingMode);
export const applyRuntimePreset = (presetId: string) =>
  useWorkbenchStore.getState().applyRuntimePreset(presetId);
```

- [ ] **Step 5: Verify Task 5**

Run:

```bash
npm test --prefix apps/desktop -- --run App.test.tsx
```

Expected: composer test and existing app shell tests pass.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src/components/Composer.tsx apps/desktop/src/stores/workbenchStore.ts apps/desktop/src/App.test.tsx
git commit -m "feat: add composer runtime controls"
```

## Task 6: Settings Runtime, MCP, and Skills Panels

**Files:**
- Modify: `apps/desktop/src/components/SettingsDialog.tsx`
- Create: `apps/desktop/src/components/settings/RuntimeSettingsPanel.tsx`
- Create: `apps/desktop/src/components/settings/McpSettingsPanel.tsx`
- Create: `apps/desktop/src/components/settings/SkillsSettingsPanel.tsx`
- Modify: `apps/desktop/src/App.test.tsx`

- [ ] **Step 1: Add failing settings tests**

In `apps/desktop/src/App.test.tsx`, add:

```ts
it("shows runtime mcp and skills settings tabs", async () => {
  render(<App />);

  await screen.findByText("Session restored");
  await userEvent.click(screen.getByRole("button", { name: "Open settings" }));

  expect(screen.getByRole("tab", { name: "Providers" })).toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "Runtime" })).toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "MCP" })).toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "Skills" })).toBeInTheDocument();

  await userEvent.click(screen.getByRole("tab", { name: "MCP" }));
  expect(screen.getByRole("button", { name: "Add MCP server" })).toBeInTheDocument();

  await userEvent.click(screen.getByRole("tab", { name: "Skills" }));
  expect(screen.getByRole("button", { name: "Add skill root" })).toBeInTheDocument();
});
```

- [ ] **Step 2: Run failing test**

Run:

```bash
npm test --prefix apps/desktop -- --run App.test.tsx
```

Expected: failure because tabs do not exist.

- [ ] **Step 3: Add settings section state**

In `SettingsDialog.tsx`, add:

```ts
type SettingsSection = "providers" | "runtime" | "mcp" | "skills";
const [section, setSection] = useState<SettingsSection>("providers");
```

Replace the single provider tab button with four buttons generated from:

```ts
const settingsSections: Array<{ id: SettingsSection; label: string; icon: typeof Settings2 }> = [
  { id: "providers", label: "Providers", icon: Settings2 },
  { id: "runtime", label: "Runtime", icon: Gauge },
  { id: "mcp", label: "MCP", icon: Server },
  { id: "skills", label: "Skills", icon: Sparkles }
];
```

Each button must set:

```tsx
role="tab"
aria-selected={section === item.id}
aria-controls={`settings-panel-${item.id}`}
onClick={() => setSection(item.id)}
```

- [ ] **Step 4: Render panels by selected section**

Keep the existing providers content in a function named `renderProvidersPanel()`. Then render:

```tsx
{section === "providers" ? renderProvidersPanel() : null}
{section === "runtime" ? <RuntimeSettingsPanel /> : null}
{section === "mcp" ? <McpSettingsPanel /> : null}
{section === "skills" ? <SkillsSettingsPanel /> : null}
```

Import the three new panel components.

- [ ] **Step 5: Create RuntimeSettingsPanel**

Create `apps/desktop/src/components/settings/RuntimeSettingsPanel.tsx`:

```tsx
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useWorkbenchStore } from "@/stores/workbenchStore";
import type { ThinkingMode } from "@/types";

const thinkingModes: ThinkingMode[] = ["auto", "low", "medium", "high"];

export function RuntimeSettingsPanel() {
  const runtimeSettings = useWorkbenchStore((state) => state.runtimeSettings);
  const saveRuntimeSettings = useWorkbenchStore((state) => state.saveRuntimeSettings);
  const [model, setModel] = useState(runtimeSettings?.default_model ?? "");
  const [thinkingMode, setThinkingMode] = useState<ThinkingMode | null>(
    runtimeSettings?.default_thinking_mode ?? "auto"
  );

  return (
    <section id="settings-panel-runtime" role="tabpanel" className="space-y-5">
      <div>
        <h2 className="text-[22px] font-semibold text-ink">Runtime</h2>
        <p className="mt-1 text-sm text-muted">Set defaults used by new chat turns.</p>
      </div>
      <label className="grid gap-2 text-sm font-medium text-muted">
        Default model
        <Input value={model} onChange={(event) => setModel(event.target.value)} />
      </label>
      <div className="grid gap-2">
        <span className="text-sm font-medium text-muted">Default thinking mode</span>
        <div className="flex flex-wrap gap-2">
          {thinkingModes.map((mode) => (
            <Button
              key={mode}
              type="button"
              variant={thinkingMode === mode ? "secondary" : "ghost"}
              aria-pressed={thinkingMode === mode}
              onClick={() => setThinkingMode(mode)}
            >
              {mode}
            </Button>
          ))}
        </div>
      </div>
      <Button
        type="button"
        onClick={() =>
          saveRuntimeSettings({
            default_model: model,
            default_thinking_mode: thinkingMode,
            presets: runtimeSettings?.presets ?? [],
            mcp_servers: runtimeSettings?.mcp_servers ?? [],
            skill_roots: runtimeSettings?.skill_roots ?? []
          })
        }
      >
        Save runtime settings
      </Button>
    </section>
  );
}
```

- [ ] **Step 6: Create McpSettingsPanel**

Create `apps/desktop/src/components/settings/McpSettingsPanel.tsx`:

```tsx
import { Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useWorkbenchStore } from "@/stores/workbenchStore";
import type { McpServerSettings } from "@/types";

export function McpSettingsPanel() {
  const runtimeSettings = useWorkbenchStore((state) => state.runtimeSettings);
  const saveRuntimeSettings = useWorkbenchStore((state) => state.saveRuntimeSettings);
  const servers = runtimeSettings?.mcp_servers ?? [];

  function saveServers(mcp_servers: McpServerSettings[]) {
    void saveRuntimeSettings({
      default_model: runtimeSettings?.default_model ?? "",
      default_thinking_mode: runtimeSettings?.default_thinking_mode ?? "auto",
      presets: runtimeSettings?.presets ?? [],
      skill_roots: runtimeSettings?.skill_roots ?? [],
      mcp_servers
    });
  }

  function addServer() {
    saveServers([
      ...servers,
      {
        id: `mcp-${Date.now()}`,
        name: "New MCP server",
        enabled: false,
        command: "npx",
        args: [],
        env: [],
        working_directory: null
      }
    ]);
  }

  function updateServer(id: string, patch: Partial<McpServerSettings>) {
    saveServers(servers.map((server) => (server.id === id ? { ...server, ...patch } : server)));
  }

  function removeServer(id: string) {
    saveServers(servers.filter((server) => server.id !== id));
  }

  return (
    <section id="settings-panel-mcp" role="tabpanel" className="space-y-5">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h2 className="text-[22px] font-semibold text-ink">MCP</h2>
          <p className="mt-1 text-sm text-muted">Configure MCP servers for the desktop runtime.</p>
        </div>
        <Button type="button" onClick={addServer}>
          <Plus className="h-4 w-4" />
          Add MCP server
        </Button>
      </div>
      <div className="space-y-3">
        {servers.length === 0 ? (
          <p className="rounded-lg border border-border bg-surface-1 p-4 text-sm text-muted">
            No MCP servers configured.
          </p>
        ) : (
          servers.map((server) => (
            <div key={server.id} className="rounded-lg border border-border bg-surface-1 p-3">
              <div className="flex items-center justify-between gap-3">
                <div className="min-w-0">
                  <p className="truncate text-sm font-medium text-ink">{server.name}</p>
                  <p className="truncate font-mono text-xs text-muted">{server.command}</p>
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  aria-label={`Remove ${server.name}`}
                  onClick={() => removeServer(server.id)}
                >
                  <Trash2 className="h-4 w-4" />
                </Button>
              </div>
              <div className="mt-3 grid gap-3 sm:grid-cols-2">
                <label className="grid gap-1 text-xs font-medium text-muted">
                  Name
                  <Input
                    value={server.name}
                    aria-label={`${server.name} name`}
                    onChange={(event) => updateServer(server.id, { name: event.target.value })}
                  />
                </label>
                <label className="grid gap-1 text-xs font-medium text-muted">
                  Enabled
                  <input
                    type="checkbox"
                    checked={server.enabled}
                    aria-label={`${server.name} enabled`}
                    onChange={(event) => updateServer(server.id, { enabled: event.target.checked })}
                  />
                </label>
                <label className="grid gap-1 text-xs font-medium text-muted">
                  Command
                  <Input
                    value={server.command}
                    aria-label={`${server.name} command`}
                    onChange={(event) => updateServer(server.id, { command: event.target.value })}
                  />
                </label>
                <label className="grid gap-1 text-xs font-medium text-muted">
                  Args
                  <Input
                    value={server.args.join(" ")}
                    aria-label={`${server.name} args`}
                    onChange={(event) =>
                      updateServer(server.id, {
                        args: event.target.value.split(/\s+/).filter(Boolean)
                      })
                    }
                  />
                </label>
                <label className="grid gap-1 text-xs font-medium text-muted sm:col-span-2">
                  Working directory
                  <Input
                    value={server.working_directory ?? ""}
                    aria-label={`${server.name} working directory`}
                    onChange={(event) =>
                      updateServer(server.id, {
                        working_directory: event.target.value.trim() || null
                      })
                    }
                  />
                </label>
                <label className="grid gap-1 text-xs font-medium text-muted sm:col-span-2">
                  Environment
                  <Input
                    value={server.env.map(([key, value]) => `${key}=${value}`).join(" ")}
                    aria-label={`${server.name} env`}
                    onChange={(event) =>
                      updateServer(server.id, {
                        env: event.target.value
                          .split(/\s+/)
                          .map((entry) => entry.split("="))
                          .filter(([key, value]) => key && value !== undefined)
                          .map(([key, ...value]) => [key, value.join("=")] as [string, string])
                      })
                    }
                  />
                </label>
              </div>
            </div>
          ))
        )}
      </div>
    </section>
  );
}
```

- [ ] **Step 7: Create SkillsSettingsPanel**

Create `apps/desktop/src/components/settings/SkillsSettingsPanel.tsx`:

```tsx
import { FolderPlus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useWorkbenchStore } from "@/stores/workbenchStore";
import type { SkillRootSettings } from "@/types";

export function SkillsSettingsPanel() {
  const runtimeSettings = useWorkbenchStore((state) => state.runtimeSettings);
  const saveRuntimeSettings = useWorkbenchStore((state) => state.saveRuntimeSettings);
  const roots = runtimeSettings?.skill_roots ?? [];

  function saveRoots(skill_roots: SkillRootSettings[]) {
    void saveRuntimeSettings({
      default_model: runtimeSettings?.default_model ?? "",
      default_thinking_mode: runtimeSettings?.default_thinking_mode ?? "auto",
      presets: runtimeSettings?.presets ?? [],
      mcp_servers: runtimeSettings?.mcp_servers ?? [],
      skill_roots
    });
  }

  function addRoot() {
    saveRoots([
      ...roots,
      {
        id: `skill-${Date.now()}`,
        name: "Local skills",
        enabled: false,
        path: "",
        scope: "global"
      }
    ]);
  }

  function updateRoot(id: string, patch: Partial<SkillRootSettings>) {
    saveRoots(roots.map((root) => (root.id === id ? { ...root, ...patch } : root)));
  }

  function removeRoot(id: string) {
    saveRoots(roots.filter((root) => root.id !== id));
  }

  return (
    <section id="settings-panel-skills" role="tabpanel" className="space-y-5">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h2 className="text-[22px] font-semibold text-ink">Skills</h2>
          <p className="mt-1 text-sm text-muted">Configure skill roots visible to ExAgent Desktop.</p>
        </div>
        <Button type="button" onClick={addRoot}>
          <FolderPlus className="h-4 w-4" />
          Add skill root
        </Button>
      </div>
      <div className="space-y-3">
        {roots.length === 0 ? (
          <p className="rounded-lg border border-border bg-surface-1 p-4 text-sm text-muted">
            No skill roots configured.
          </p>
        ) : (
          roots.map((root) => (
            <div key={root.id} className="rounded-lg border border-border bg-surface-1 p-3">
              <div className="flex items-center justify-between gap-3">
                <div className="min-w-0">
                  <p className="truncate text-sm font-medium text-ink">{root.name}</p>
                  <p className="truncate font-mono text-xs text-muted">{root.path || "Path not set"}</p>
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  aria-label={`Remove ${root.name}`}
                  onClick={() => removeRoot(root.id)}
                >
                  <Trash2 className="h-4 w-4" />
                </Button>
              </div>
              <div className="mt-3 grid gap-3 sm:grid-cols-2">
                <label className="grid gap-1 text-xs font-medium text-muted">
                  Name
                  <Input
                    value={root.name}
                    aria-label={`${root.name} name`}
                    onChange={(event) => updateRoot(root.id, { name: event.target.value })}
                  />
                </label>
                <label className="grid gap-1 text-xs font-medium text-muted">
                  Enabled
                  <input
                    type="checkbox"
                    checked={root.enabled}
                    aria-label={`${root.name} enabled`}
                    onChange={(event) => updateRoot(root.id, { enabled: event.target.checked })}
                  />
                </label>
                <label className="grid gap-1 text-xs font-medium text-muted">
                  Path
                  <Input
                    value={root.path}
                    aria-label={`${root.name} path`}
                    onChange={(event) => updateRoot(root.id, { path: event.target.value })}
                  />
                </label>
                <label className="grid gap-1 text-xs font-medium text-muted">
                  Scope
                  <select
                    className="h-10 rounded-md border border-border bg-surface-2 px-2 text-sm text-ink"
                    value={root.scope}
                    aria-label={`${root.name} scope`}
                    onChange={(event) => updateRoot(root.id, { scope: event.target.value })}
                  >
                    <option value="global">global</option>
                    <option value="project">project</option>
                  </select>
                </label>
              </div>
            </div>
          ))
        )}
      </div>
    </section>
  );
}
```

- [ ] **Step 8: Verify Task 6**

Run:

```bash
npm test --prefix apps/desktop -- --run App.test.tsx
```

Expected: settings tab tests pass.

- [ ] **Step 9: Commit**

```bash
git add apps/desktop/src/components/SettingsDialog.tsx apps/desktop/src/components/settings/RuntimeSettingsPanel.tsx apps/desktop/src/components/settings/McpSettingsPanel.tsx apps/desktop/src/components/settings/SkillsSettingsPanel.tsx apps/desktop/src/App.test.tsx
git commit -m "feat: add runtime mcp and skills settings panels"
```

## Task 7: Inspector Runtime Visibility

**Files:**
- Modify: `apps/desktop/src/components/Inspector.tsx`
- Modify: `apps/desktop/src/App.test.tsx`

- [ ] **Step 1: Add failing inspector test**

In `apps/desktop/src/App.test.tsx`, add:

```ts
it("shows runtime controls and configuration counts in inspector", async () => {
  render(<App />);

  await screen.findByText("Session restored");

  expect(screen.getByText("Runtime")).toBeInTheDocument();
  expect(screen.getByText("model")).toBeInTheDocument();
  expect(screen.getByText("thinking")).toBeInTheDocument();
  expect(screen.getByText("MCP servers")).toBeInTheDocument();
  expect(screen.getByText("Skill roots")).toBeInTheDocument();
});
```

- [ ] **Step 2: Run failing test**

Run:

```bash
npm test --prefix apps/desktop -- --run App.test.tsx
```

Expected: failure because inspector runtime section does not exist.

- [ ] **Step 3: Add runtime section**

In `Inspector.tsx`, import `Cpu` or another existing lucide icon:

```ts
import { Activity, Cpu, FileText, Gauge, HardDrive, ShieldCheck } from "lucide-react";
```

Before `Token Usage`, add:

```tsx
<InspectorSection icon={Cpu} title="Runtime">
  <KeyValue label="model" value={state.selectedModel ?? state.runtimeSettings?.default_model ?? "default"} mono />
  <KeyValue label="thinking" value={state.selectedThinkingMode ?? state.runtimeSettings?.default_thinking_mode ?? "auto"} />
  <KeyValue
    label="MCP servers"
    value={`${state.runtimeSettings?.mcp_servers.filter((server) => server.enabled).length ?? 0} enabled`}
  />
  <KeyValue
    label="Skill roots"
    value={`${state.runtimeSettings?.skill_roots.filter((root) => root.enabled).length ?? 0} enabled`}
  />
</InspectorSection>
```

- [ ] **Step 4: Verify Task 7**

Run:

```bash
npm test --prefix apps/desktop -- --run App.test.tsx
```

Expected: inspector test passes.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/components/Inspector.tsx apps/desktop/src/App.test.tsx
git commit -m "feat: show runtime config in inspector"
```

## Task 8: Final Verification

**Files:**
- Verify all files modified by Tasks 1-7.

- [ ] **Step 1: Format Rust**

Run:

```bash
cargo fmt --check
```

Expected: passes.

- [ ] **Step 2: Run Rust tests**

Run:

```bash
cargo test
```

Expected: passes.

- [ ] **Step 3: Check diff whitespace**

Run:

```bash
git diff --check
```

Expected: passes.

- [ ] **Step 4: Run desktop tests**

Run:

```bash
npm test --prefix apps/desktop
```

Expected: passes.

- [ ] **Step 5: Build desktop frontend**

Run:

```bash
npm run build --prefix apps/desktop
```

Expected: Vite build passes.

- [ ] **Step 6: Manual GUI acceptance**

Run:

```bash
npm run tauri:dev --prefix apps/desktop
```

Verify:

- Composer shows model input, thinking buttons, and preset dropdown when presets exist.
- Sending a prompt from Tauri includes selected model and thinking mode in `turn_start`.
- Settings opens with Providers, Runtime, MCP, and Skills tabs.
- Runtime defaults save and update Composer defaults.
- MCP server entries can be added, removed, saved, and reloaded.
- Skill root entries can be added, removed, saved, and reloaded.
- Inspector shows effective model, thinking mode, enabled MCP server count, and enabled skill root count.
- There is no Diagnostics/Safety tab.

- [ ] **Step 7: Final commit**

```bash
git status --short
git add src apps/desktop docs/superpowers/plans/2026-06-01-desktop-runtime-mcp-skills-gui.md
git commit -m "feat: add desktop runtime configuration gui"
```

## Self-Review Checklist

- Spec coverage: Tasks 1, 3, 4, and 5 cover chat model/thinking controls; Tasks 2, 3, and 6 cover MCP/Skills configuration; Task 7 covers inspector visibility; Task 8 covers acceptance.
- Placeholder scan: this plan contains no placeholder requirements and no blank implementation steps.
- Scope check: Diagnostics/Safety, MCP process launching, and skill execution are explicitly out of scope.
- Type consistency: `ThinkingMode`, `RuntimeSettingsResponse`, `RuntimeSettingsSaveRequest`, `McpServerSettings`, and `SkillRootSettings` are introduced before frontend use.
