export type SessionStatus = "idle" | "running" | "awaiting_approval" | "failed" | "archived";

export type ThinkingMode = "auto" | "low" | "medium" | "high";

export interface ProjectSummary {
  id: string;
  name: string;
  path: string;
  active?: boolean;
}

export interface SessionSummary {
  id: string;
  projectId: string;
  title: string;
  updatedAt: string;
  status: SessionStatus;
  pinned?: boolean;
  archived?: boolean;
}

export interface TranscriptMessage {
  id: string;
  role: "user" | "assistant" | "system" | "tool" | "approval";
  title?: string;
  body: string;
  timestamp: string;
  status?: "info" | "success" | "warning" | "danger";
  threadId?: string;
  turnId?: string;
  approvalId?: string;
  toolName?: string;
}

export interface RuntimeEvent {
  id: string;
  label: string;
  detail: string;
  timestamp: string;
  tone?: "neutral" | "info" | "warning" | "danger" | "success";
}

export interface ChangedFile {
  path: string;
  status: "modified" | "added" | "deleted";
}

export interface WorkbenchSnapshot {
  projects: ProjectSummary[];
  sessions: SessionSummary[];
  activeProjectId: string | null;
  activeSessionId: string | null;
  transcript: TranscriptMessage[];
  events: RuntimeEvent[];
  changedFiles: ChangedFile[];
  cwd: string;
  policy: string;
  tokenUsage: {
    input: number;
    output: number;
    limit: number;
  };
  runtimeSettings: RuntimeSettingsResponse | null;
  selectedModel: ModelRef | null;
  selectedThinkingMode: ThinkingMode | null;
}

export type ModelRef = {
  provider_id: string;
  model_id: string;
};

export interface ProjectRecord {
  id: string;
  name: string;
  path: string;
}

export interface ThreadRecord {
  id: string;
  project_id: string;
  rollout_path: string;
  user_title: string | null;
  fallback_title: string;
  preview: string;
  title_source: string;
  archived_at: number | null;
  pinned: boolean;
  status: string;
  created_at: number;
  updated_at: number;
  last_opened_at: number | null;
}

export interface ThreadView {
  id: string;
  status: string;
  active_turn: TurnView | null;
  turns: TurnView[];
}

export interface TurnView {
  id: string;
  status: string;
  items: ThreadItem[];
}

export type ThreadItem =
  | { type: "user_message"; text: string }
  | { type: "assistant_message"; text: string | null }
  | { type: "tool_result"; name: string }
  | { type: "exec_output"; text: string }
  | { type: "approval_requested"; approval_id: string; tool_name: string; reason: string }
  | { type: "approval_decision"; status: string; note: string | null }
  | { type: "runtime_error"; message: string }
  | { type: "compaction_written" };

export interface ThreadReadResponse {
  thread: ThreadView;
}

export interface ThreadStartResponse {
  thread: ThreadView;
}

export interface TurnStartResponse {
  thread_id: string;
  turn: TurnView;
}

export interface EventsReplayResponse {
  thread_id: string;
  events: BackendRuntimeEvent[];
  snapshot?: unknown;
}

export interface BackendRuntimeEvent {
  event_id: string;
  thread_id: string;
  turn_id?: string | null;
  kind: BackendRuntimeEventKind;
}

export type BackendRuntimeEventKind =
  | { type: "turn_started" }
  | { type: "turn_completed" }
  | { type: "turn_interrupted" }
  | { type: "assistant_turn"; turn: { text: string | null; tool_calls: unknown[] } }
  | { type: "tool_result"; result: { tool_name: string; content: string; status: string; meta?: unknown } }
  | { type: "exec_output"; exec_session_id: string; stream: string; chunk: string }
  | { type: "approval_requested"; approval_id: string; tool_name: string; reason: string }
  | { type: "approval_decision"; approval_id: string; status: string; note: string | null }
  | { type: "compaction_written"; summary: { summary: string } }
  | { type: "token_count"; info?: unknown }
  | { type: "runtime_error"; message: string };

export interface ProviderDescriptor {
  id: string;
  name: string;
  description: string;
  recommended: boolean;
  supported: boolean;
  auth_mode: ProviderAuthMode;
  protocol: ProviderProtocol;
  default_base_url: string;
  default_model: string;
  supports_model_discovery: boolean;
  supports_tools: boolean;
  unsupported_reason: string | null;
}

export type ProviderAuthMode = "api_key_required" | "api_key_optional" | "oauth_planned";

export type ProviderProtocol =
  | "openai_chat_completions"
  | "anthropic_messages"
  | "gemini_generate_content"
  | "copilot_oauth";

export type CredentialSource = "keychain" | "environment" | "none";

export interface ProviderConfigView {
  provider_id: string;
  base_url: string;
  model: string;
  has_api_key: boolean;
  credential_source: CredentialSource;
  auth_required: boolean;
}

export interface ConnectedProviderView {
  id: string;
  name: string;
  model: string;
  base_url: string;
}

export interface ProviderSettingsResponse {
  providers: ProviderDescriptor[];
  active_provider_id: string;
  config: ProviderConfigView;
  connected_provider: ConnectedProviderView | null;
  last_connection: ProviderConnectionStatusView | null;
}

export interface ProviderSettingsSaveRequest {
  providerId: string;
  baseUrl: string;
  model: string;
  apiKey: string | null;
  clearApiKey: boolean;
}

export type ProviderConnectionStatus =
  | "success"
  | "unsupported_provider"
  | "missing_credential"
  | "authentication_failed"
  | "model_not_found"
  | "network_error"
  | "provider_error";

export interface ProviderConnectionTestRequest {
  providerId: string;
  baseUrl: string;
  model: string;
  apiKey: string | null;
  useSavedApiKey: boolean;
}

export interface ProviderConnectionTestResponse {
  status: ProviderConnectionStatus;
  message: string;
}

export interface ProviderConnectionStatusView extends ProviderConnectionTestResponse {
  checked_at: string;
}

export interface ProviderModelListRequest {
  providerId: string;
  baseUrl: string;
  apiKey: string | null;
  useSavedApiKey: boolean;
}

export type ProviderModelListStatus =
  | "success"
  | "unsupported_provider"
  | "missing_credential"
  | "unavailable"
  | "authentication_failed"
  | "network_error"
  | "provider_error";

export interface ProviderModelView {
  id: string;
  display_name: string;
  context_window: number | null;
  supports_tools: boolean | null;
}

export interface ProviderModelListResponse {
  status: ProviderModelListStatus;
  message: string;
  models: ProviderModelView[];
}

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
