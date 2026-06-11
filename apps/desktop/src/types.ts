export type SessionStatus = "idle" | "running" | "awaiting_approval" | "failed" | "archived";

export type ThinkingMode = "auto" | "off" | "minimal" | "low" | "medium" | "high" | "x_high";
export type TurnMode = "default" | "plan";
export type ImageDetail = "auto" | "low" | "high" | "original";
export type InputModality = "text" | "image";

export type TurnInput =
  | { type: "text"; text: string }
  | { type: "local_image"; path: string; detail?: ImageDetail | null }
  | { type: "image_url"; url: string; detail?: ImageDetail | null };

export interface ComposerAttachment {
  id: string;
  type: "local_image";
  path: string;
  name: string;
  detail: ImageDetail;
}

export interface ThinkingCapability {
  supported: boolean;
  modes: Exclude<ThinkingMode, "auto">[];
}

export type ReasoningProtocol =
  | "none"
  | "open_ai_reasoning_effort"
  | "deep_seek_thinking"
  | "thinking_object"
  | "open_router_reasoning_object"
  | "zai_thinking_object"
  | "qwen_chat_template"
  | "gemini_thinking_config"
  | "anthropic_thinking_budget";

export interface ReasoningCapabilities {
  protocol: ReasoningProtocol;
  supported_modes: Exclude<ThinkingMode, "auto">[];
  default_mode?: Exclude<ThinkingMode, "auto"> | null;
}

export interface ModelCapabilities {
  supports_tools: boolean;
  thinking: ThinkingCapability;
  reasoning?: ReasoningCapabilities | null;
  input_modalities?: InputModality[];
}

export interface ProjectSummary {
  id: string;
  name: string;
  path: string;
  active?: boolean;
  pinned?: boolean;
  archived?: boolean;
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
  role: "user" | "assistant" | "reasoning" | "system" | "tool" | "approval";
  title?: string;
  body: string;
  input?: TurnInput[];
  timestamp: string;
  status?: "info" | "success" | "warning" | "danger";
  threadId?: string;
  turnId?: string;
  approvalId?: string;
  invocationId?: string;
  toolCallId?: string;
  toolName?: string;
  toolStatus?: ToolInvocationTranscriptStatus;
  mutating?: boolean;
}

export type ToolInvocationTranscriptStatus =
  | "running"
  | "waiting_approval"
  | "completed"
  | "failed"
  | "cancelled";

export interface RuntimeEvent {
  id: string;
  label: string;
  detail: string;
  timestamp: string;
  tone?: "neutral" | "info" | "warning" | "danger" | "success";
}

export interface AgentThreadView {
  threadId: string;
  transcript: TranscriptMessage[];
  events: RuntimeEvent[];
  loading: boolean;
  error: string | null;
}

export interface ChangedFile {
  path: string;
  status: "modified" | "added" | "deleted";
}

export type AgentRunStatus = "running" | "spawning" | "done" | "idle" | "failed";
export type AgentType = "explorer" | "planner" | "reviewer" | "worker";

export interface AgentNode {
  threadId: string;
  parentThreadId: string | null;
  name: string;
  agentPath: string | null;
  status: AgentRunStatus;
  task: string;
  lastActivity: string | null;
  agentType?: AgentType | null;
  role?: string | null;
  nickname?: string | null;
  isRoot: boolean;
  children: AgentNode[];
}

export type AgentTreeNodeStatus = "idle" | "running" | "done" | "failed";

export interface AgentTreeNode {
  thread_id?: string | null;
  parent_thread_id?: string | null;
  root_thread_id: string;
  depth: number;
  agent_path: string;
  status: AgentTreeNodeStatus;
  agent_type?: AgentType | null;
  agent_role?: string | null;
  agent_nickname?: string | null;
  last_task_message?: string | null;
  last_activity?: string | null;
  children?: AgentTreeNode[];
}

export interface AgentTreeResponse {
  root: AgentTreeNode;
}

export interface TokenUsageCounts {
  input_tokens: number;
  cached_input_tokens: number;
  output_tokens: number;
  reasoning_output_tokens: number;
  total_tokens: number;
}

export interface TokenUsageInfo {
  total_token_usage: TokenUsageCounts;
  last_token_usage: TokenUsageCounts;
  model_context_window: number | null;
}

export interface ThreadTokenUsage {
  threadId: string;
  total: TokenUsageCounts;
  last: TokenUsageCounts;
  modelContextWindow: number | null;
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
  tokenUsageByThreadId: Record<string, ThreadTokenUsage>;
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
  archived_at: number | null;
  pinned: boolean;
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
  goal?: ThreadGoal | null;
}

export type ThreadGoalStatus =
  | "active"
  | "paused"
  | "blocked"
  | "usage_limited"
  | "budget_limited"
  | "complete";

export interface ThreadGoal {
  thread_id: string;
  goal_id: string;
  objective: string;
  status: ThreadGoalStatus;
  token_budget?: number | null;
  tokens_used: number;
  time_used_seconds: number;
  continuation_suppressed: boolean;
  continuation_suppressed_after_turn_id?: string | null;
  created_at_ms: number;
  updated_at_ms: number;
}

export interface DraftThreadGoal {
  objective: string;
  token_budget: number | null;
}

export interface ThreadGoalSetResponse {
  goal: ThreadGoal;
}

export interface ThreadGoalGetResponse {
  goal: ThreadGoal | null;
}

export interface ThreadGoalClearResponse {
  cleared: boolean;
}

export interface TurnView {
  id: string;
  status: string;
  items: ThreadItem[];
}

export type ThreadItem =
  | { type: "user_message"; text: string; input?: TurnInput[] }
  | { type: "assistant_message"; event_id?: string | null; text: string | null }
  | { type: "reasoning"; event_id?: string | null; summary?: string[]; content?: string[] }
  | { type: "tool_result"; event_id?: string | null; name: string }
  | {
      type: "tool_invocation";
      invocation_id: string;
      tool_call_id?: string | null;
      tool_name?: string | null;
      approval_id?: string | null;
      status: string;
      mutating?: boolean | null;
      reason?: string | null;
      message?: string | null;
      output_preview?: string | null;
    }
  | { type: "exec_output"; event_id?: string | null; text: string }
  | { type: "approval_requested"; event_id?: string | null; approval_id: string; tool_name: string; reason: string }
  | { type: "approval_decision"; event_id?: string | null; approval_id?: string | null; status: string; note: string | null }
  | { type: "runtime_error"; event_id?: string | null; message: string }
  | {
      type: "subagent_spawn";
      event_id?: string | null;
      invocation_id: string;
      tool_call_id: string;
      parent_thread_id: string;
      child_thread_id: string;
      task_name: string;
      message_preview: string;
    }
  | {
      type: "subagent_close";
      event_id?: string | null;
      invocation_id: string;
      tool_call_id: string;
      parent_thread_id: string;
      closed_thread_id: string;
      agent_path: string;
    }
  | {
      type: "inter_agent_message";
      event_id?: string | null;
      invocation_id: string;
      tool_call_id: string;
      author_thread_id: string;
      recipient_thread_id: string;
      author_path: string;
      recipient_path: string;
      content_preview: string;
      followup: boolean;
      started_turn_id?: string | null;
    }
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
  | { type: "assistant_text_delta"; delta: string }
  | { type: "assistant_turn"; turn: { text: string | null; tool_calls: unknown[] } }
  | { type: "reasoning_delta"; delta: string }
  | { type: "reasoning"; summary?: string[]; content?: string[] }
  | { type: "tool_result"; result: { tool_call_id: string; tool_name: string; content: string; status: string; meta?: unknown } }
  | { type: "tool_invocation_started"; invocation_id: string; tool_call_id: string; tool_name: string; mutating: boolean }
  | { type: "tool_invocation_waiting_approval"; invocation_id: string; approval_id: string; reason: string }
  | { type: "tool_invocation_output_delta"; invocation_id: string; stream: "stdout" | "stderr"; chunk: string; sequence: number }
  | { type: "tool_invocation_completed"; invocation_id: string; tool_call_id: string; tool_name: string; status: string }
  | { type: "tool_invocation_failed"; invocation_id: string; tool_call_id: string; tool_name: string; message: string }
  | { type: "tool_invocation_cancelled"; invocation_id: string; tool_call_id: string; tool_name: string; reason: string }
  | { type: "exec_output"; exec_session_id: string; stream: "stdout" | "stderr"; chunk: string; sequence?: number }
  | { type: "approval_requested"; approval_id: string; tool_name: string; reason: string }
  | { type: "approval_decision"; approval_id: string; status: string; note: string | null }
  | { type: "compaction_written"; summary: { summary: string } }
  | {
      type: "subagent_spawned";
      invocation_id: string;
      tool_call_id: string;
      parent_thread_id: string;
      child_thread_id: string;
      task_name: string;
      message_preview: string;
    }
  | {
      type: "subagent_closed";
      invocation_id: string;
      tool_call_id: string;
      parent_thread_id: string;
      closed_thread_id: string;
      agent_path: string;
    }
  | {
      type: "inter_agent_message_sent";
      invocation_id: string;
      tool_call_id: string;
      author_thread_id: string;
      recipient_thread_id: string;
      author_path: string;
      recipient_path: string;
      content_preview: string;
      followup: boolean;
      started_turn_id?: string | null;
    }
  | { type: "thread_goal_updated"; goal: ThreadGoal }
  | { type: "thread_goal_cleared"; thread_id: string }
  | { type: "thread_goal_continuation_started"; goal_id: string }
  | { type: "thread_goal_continuation_suppressed"; goal_id: string; reason: string }
  | { type: "token_count"; info?: TokenUsageInfo | null }
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

export type ProviderAuthMode = "api_key_required" | "api_key_optional" | "oauth_required" | "oauth_planned";

export type ProviderProtocol =
  | "openai_chat_completions"
  | "anthropic_messages"
  | "gemini_generate_content"
  | "copilot_oauth";

export type CredentialSource = "keychain" | "environment" | "none";
export type CredentialKind = "api_key" | "oauth";
export type CredentialStatus = "active" | "expired" | "needs_login";
export type CredentialAuthMethod = "chatgpt_oauth" | "github_copilot_oauth";

export interface ProviderConfigView {
  provider_id: string;
  base_url: string;
  model: string;
  has_api_key: boolean;
  has_credential?: boolean;
  credential_kind?: CredentialKind | null;
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
  active_credential_id?: string | null;
  credentials?: ProviderCredentialView[];
  config: ProviderConfigView;
  connected_provider: ConnectedProviderView | null;
  last_connection: ProviderConnectionStatusView | null;
  configured_providers: ProviderConfigView[];
  model_options: ProviderModelView[];
}

export interface ProviderCredentialView {
  id: string;
  label: string;
  source: CredentialSource;
  kind: CredentialKind;
  status: CredentialStatus;
  auth_method?: CredentialAuthMethod | null;
  account_label?: string | null;
}

export interface ProviderSettingsSaveRequest {
  providerId: string;
  baseUrl: string;
  model: string;
  apiKey: string | null;
  clearApiKey: boolean;
  credentialId?: string | null;
  createCredential?: boolean;
  modelOptions?: ProviderModelView[];
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
  provider_id: string;
  id: string;
  display_name: string;
  context_window: number | null;
  supports_tools: boolean | null;
  capabilities: ModelCapabilities;
}

export interface ProviderModelListResponse {
  status: ProviderModelListStatus;
  message: string;
  models: ProviderModelView[];
}

export interface ChatGptDeviceCode {
  device_auth_id: string;
  user_code: string;
  verification_uri: string;
  expires_in: number;
  interval: number;
}

export interface GitHubCopilotDeviceCode {
  device_code: string;
  user_code: string;
  verification_uri: string;
  expires_in: number;
  interval: number;
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

export interface SkillCatalogScanResponse {
  sources: SkillSourceView[];
  skills: SkillCatalogItemView[];
  warnings: SkillCatalogWarningView[];
}

export interface SkillSourceView {
  id: string;
  name: string;
  scope: string;
  enabled: boolean;
  path: string;
  status: string;
  skill_count: number;
  warning_count: number;
}

export interface SkillCatalogItemView {
  name: string;
  scope: string;
  description: string;
  path: string;
  source_id: string;
  allow_implicit_invocation: boolean;
  effective_implicit: boolean;
  status: string;
}

export interface SkillCatalogWarningView {
  kind: string;
  scope: string;
  name: string;
  paths: string[];
}

export interface RuntimeSettingsResponse {
  default_model: string;
  default_thinking_mode: ThinkingMode | null;
  presets: RuntimePresetSettings[];
  mcp_servers: McpServerSettings[];
  skill_roots: SkillRootSettings[];
}

export type RuntimeSettingsSaveRequest = RuntimeSettingsResponse;
