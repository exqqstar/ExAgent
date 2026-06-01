import type {
  BackendRuntimeEvent,
  EventsReplayResponse,
  ModelRef,
  ProviderSettingsResponse,
  ProviderSettingsSaveRequest,
  ProviderConnectionTestRequest,
  ProviderConnectionTestResponse,
  ProviderModelListRequest,
  ProviderModelListResponse,
  ProjectRecord,
  RuntimeSettingsResponse,
  RuntimeSettingsSaveRequest,
  SessionStatus,
  SessionSummary,
  ThinkingMode,
  ThreadReadResponse,
  ThreadRecord,
  ThreadStartResponse,
  TurnStartResponse,
  WorkbenchSnapshot
} from "@/types";

type Unlisten = () => void;

const mockSnapshot: WorkbenchSnapshot = {
  projects: [
    {
      id: "project-exagent",
      name: "ExAgent",
      path: "/Volumes/EXEXEX/ExAgent",
      active: true
    }
  ],
  sessions: [
    {
      id: "session-desktop",
      projectId: "project-exagent",
      title: "Desktop GUI workbench",
      updatedAt: "local preview",
      status: "running"
    }
  ],
  activeProjectId: "project-exagent",
  activeSessionId: "session-desktop",
  transcript: [
    {
      id: "message-system",
      role: "system",
      title: "Session restored",
      body: "Project context loaded from the local workspace.",
      timestamp: "preview",
      status: "info"
    },
    {
      id: "message-user",
      role: "user",
      body: "Create the initial desktop frontend scaffold only.",
      timestamp: "preview"
    },
    {
      id: "message-assistant",
      role: "assistant",
      title: "Working",
      body: "Scaffolding the React workbench shell, local UI primitives, and runtime command bridge.",
      timestamp: "preview",
      status: "success"
    }
  ],
  events: [
    {
      id: "event-preview",
      label: "Preview",
      detail: "Tauri commands are used when the app runs in the desktop shell.",
      timestamp: "preview",
      tone: "info"
    }
  ],
  changedFiles: [],
  cwd: "/Volumes/EXEXEX/ExAgent",
  policy: "local",
  tokenUsage: {
    input: 0,
    output: 0,
    limit: 1
  },
  runtimeSettings: null,
  selectedModel: null,
  selectedThinkingMode: null
};

const mockProviderSettings: ProviderSettingsResponse = {
  providers: [
    {
      id: "openai",
      name: "OpenAI",
      description: "Use ChatGPT Pro/Plus or an API key",
      recommended: true,
      supported: true,
      auth_mode: "api_key_required",
      protocol: "openai_chat_completions",
      default_base_url: "https://api.openai.com/v1",
      default_model: "gpt-4.1",
      supports_model_discovery: true,
      supports_tools: true,
      unsupported_reason: null
    },
    {
      id: "openai_compatible",
      name: "OpenAI Compatible",
      description: "Use OpenRouter, DeepSeek, local gateways, or another compatible endpoint",
      recommended: true,
      supported: true,
      auth_mode: "api_key_optional",
      protocol: "openai_chat_completions",
      default_base_url: "http://127.0.0.1:11434/v1",
      default_model: "local-model",
      supports_model_discovery: true,
      supports_tools: true,
      unsupported_reason: null
    },
    {
      id: "anthropic",
      name: "Anthropic",
      description: "Claude API support is planned",
      recommended: false,
      supported: false,
      auth_mode: "api_key_required",
      protocol: "anthropic_messages",
      default_base_url: "https://api.anthropic.com/v1",
      default_model: "claude-sonnet",
      supports_model_discovery: false,
      supports_tools: true,
      unsupported_reason: "Anthropic Messages adapter is planned."
    },
    {
      id: "google",
      name: "Google",
      description: "Gemini API support is planned",
      recommended: false,
      supported: false,
      auth_mode: "api_key_required",
      protocol: "gemini_generate_content",
      default_base_url: "https://generativelanguage.googleapis.com/v1beta",
      default_model: "gemini-2.5-pro",
      supports_model_discovery: true,
      supports_tools: true,
      unsupported_reason: "Gemini Generate Content adapter is planned."
    },
    {
      id: "github_copilot",
      name: "GitHub Copilot",
      description: "Copilot account support is planned",
      recommended: false,
      supported: false,
      auth_mode: "oauth_planned",
      protocol: "copilot_oauth",
      default_base_url: "",
      default_model: "",
      supports_model_discovery: false,
      supports_tools: false,
      unsupported_reason: "Copilot OAuth support is planned."
    }
  ],
  active_provider_id: "openai",
  config: {
    provider_id: "openai",
    base_url: "https://api.openai.com/v1",
    model: "gpt-4.1",
    has_api_key: false,
    credential_source: "none",
    auth_required: true
  },
  connected_provider: null,
  last_connection: null
};

const mockRuntimeSettings: RuntimeSettingsResponse = {
  default_model: "gpt-4.1",
  default_thinking_mode: "auto",
  presets: [
    {
      id: "fast",
      name: "Fast",
      model: "gpt-4.1-mini",
      thinking_mode: "low"
    },
    {
      id: "deep",
      name: "Deep",
      model: "gpt-4.1",
      thinking_mode: "high"
    }
  ],
  mcp_servers: [],
  skill_roots: []
};

function isTauriRuntime() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export function isDesktopRuntime() {
  return isTauriRuntime();
}

async function invokeCommand<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(command, args);
}

async function fallbackSnapshot(): Promise<WorkbenchSnapshot> {
  return mockSnapshot;
}

export async function getWorkbenchSnapshot(): Promise<WorkbenchSnapshot> {
  if (!isTauriRuntime()) {
    return fallbackSnapshot();
  }

  const projects = await listProjects();
  const activeProject = projects[0] ?? null;
  const sessions = activeProject ? await reindexProject(activeProject.id) : [];

  return {
    projects: projects.map((project) => ({
      id: project.id,
      name: project.name,
      path: project.path,
      active: project.id === activeProject?.id
    })),
    sessions: sessions.map(threadRecordToSession),
    activeProjectId: activeProject?.id ?? null,
    activeSessionId: sessions[0]?.id ?? null,
    transcript: [],
    events: [],
    changedFiles: [],
    cwd: activeProject?.path ?? "No project selected",
    policy: "local",
    tokenUsage: {
      input: 0,
      output: 0,
      limit: 1
    },
    runtimeSettings: null,
    selectedModel: null,
    selectedThinkingMode: null
  };
}

export async function pickAndAddProject(): Promise<ProjectRecord | null> {
  if (!isTauriRuntime()) {
    return null;
  }

  const { open } = await import("@tauri-apps/plugin-dialog");
  const selected = await open({
    directory: true,
    multiple: false
  });
  if (typeof selected !== "string") {
    return null;
  }

  const name = selected.split(/[\\/]/).filter(Boolean).at(-1) ?? "Project";
  return invokeCommand<ProjectRecord>("project_add", { name, path: selected });
}

export async function listProjects(): Promise<ProjectRecord[]> {
  if (!isTauriRuntime()) {
    return mockSnapshot.projects.map((project) => ({
      id: project.id,
      name: project.name,
      path: project.path
    }));
  }
  return invokeCommand<ProjectRecord[]>("project_list");
}

export async function listThreads(
  projectId: string,
  includeArchived: boolean,
  search: string | null
): Promise<ThreadRecord[]> {
  if (!isTauriRuntime()) {
    return mockSnapshot.sessions
      .filter((session) => session.projectId === projectId)
      .map((session) => ({
        id: session.id,
        project_id: projectId,
        rollout_path: "",
        user_title: session.title,
        fallback_title: session.title,
        preview: session.title,
        title_source: "mock",
        archived_at: null,
        pinned: false,
        status: session.status,
        created_at: 0,
        updated_at: 0,
        last_opened_at: null
      }));
  }

  return invokeCommand<ThreadRecord[]>("thread_list", {
    projectId,
    includeArchived,
    search
  });
}

export async function reindexProject(projectId: string): Promise<ThreadRecord[]> {
  if (!isTauriRuntime()) {
    return listThreads(projectId, false, null);
  }

  return invokeCommand<ThreadRecord[]>("project_reindex", { projectId });
}

export async function startThread(projectId: string): Promise<ThreadStartResponse> {
  return invokeCommand<ThreadStartResponse>("thread_start", { projectId });
}

export async function readThread(projectId: string, threadId: string): Promise<ThreadReadResponse> {
  return invokeCommand<ThreadReadResponse>("thread_read", { projectId, threadId });
}

export async function resumeThread(projectId: string, threadId: string): Promise<ThreadReadResponse> {
  return invokeCommand<ThreadReadResponse>("thread_resume", { projectId, threadId });
}

export async function startTurn(
  projectId: string,
  threadId: string,
  prompt: string,
  model?: ModelRef | null,
  thinkingMode?: ThinkingMode | null
): Promise<TurnStartResponse> {
  return invokeCommand<TurnStartResponse>("turn_start", {
    projectId,
    threadId,
    prompt,
    model: model?.provider_id.trim() && model?.model_id.trim() ? model : null,
    thinkingMode: thinkingMode ?? null
  });
}

export async function interruptTurn(projectId: string, threadId: string, turnId?: string) {
  return invokeCommand("turn_interrupt", { projectId, threadId, turnId: turnId ?? null });
}

export async function renameThread(threadId: string, title: string) {
  return invokeCommand("thread_rename", { threadId, title });
}

export async function pinThread(threadId: string, pinned: boolean) {
  return invokeCommand("thread_pin", { threadId, pinned });
}

export async function archiveThread(threadId: string) {
  return invokeCommand("thread_archive", { threadId });
}

export async function submitApprovalDecision(
  projectId: string,
  threadId: string,
  turnId: string | undefined,
  approvalId: string,
  decision: "approved" | "denied",
  note?: string
) {
  return invokeCommand("approval_decision", {
    projectId,
    threadId,
    turnId: turnId ?? null,
    approvalId,
    decision,
    note: note ?? null
  });
}

export async function replayEvents(
  projectId: string,
  threadId: string,
  afterEventId?: string | null
): Promise<EventsReplayResponse> {
  return invokeCommand<EventsReplayResponse>("events_replay", {
    projectId,
    threadId,
    afterEventId: afterEventId ?? null,
    includeSnapshot: true
  });
}

export async function subscribeRuntimeEvents(
  projectId: string,
  threadId: string,
  onEvent: (event: BackendRuntimeEvent) => void
): Promise<Unlisten | null> {
  if (!isTauriRuntime()) {
    return null;
  }

  const { listen } = await import("@tauri-apps/api/event");
  const unlisten = await listen<BackendRuntimeEvent>("exagent://runtime-event", (event) => {
    onEvent(event.payload);
  });
  await invokeCommand("events_subscribe", { projectId, threadId, afterEventId: null });
  return unlisten;
}

export async function getProviderSettings(): Promise<ProviderSettingsResponse> {
  if (!isTauriRuntime()) {
    return mockProviderSettings;
  }
  return invokeCommand<ProviderSettingsResponse>("provider_settings_get");
}

export async function getRuntimeSettings(): Promise<RuntimeSettingsResponse> {
  if (!isTauriRuntime()) {
    return mockRuntimeSettings;
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

export async function saveProviderSettings(
  request: ProviderSettingsSaveRequest
): Promise<ProviderSettingsResponse> {
  if (!isTauriRuntime()) {
    return {
      ...mockProviderSettings,
      active_provider_id: request.providerId,
      config: {
        provider_id: request.providerId,
        base_url: request.baseUrl,
        model: request.model,
        has_api_key: Boolean(request.apiKey && !request.clearApiKey),
        credential_source: request.apiKey && !request.clearApiKey ? "keychain" : "none",
        auth_required:
          mockProviderSettings.providers.find((provider) => provider.id === request.providerId)?.auth_mode ===
          "api_key_required"
      },
      connected_provider:
        (request.apiKey && !request.clearApiKey) ||
        mockProviderSettings.providers.find((provider) => provider.id === request.providerId)?.auth_mode ===
          "api_key_optional"
          ? {
              id: request.providerId,
              name:
                mockProviderSettings.providers.find((provider) => provider.id === request.providerId)?.name ??
                request.providerId,
              model: request.model,
              base_url: request.baseUrl
            }
          : null,
      last_connection: null
    };
  }
  return invokeCommand<ProviderSettingsResponse>("provider_settings_save", { request });
}

export async function testProviderConnection(
  request: ProviderConnectionTestRequest
): Promise<ProviderConnectionTestResponse> {
  if (!isTauriRuntime()) {
    return {
      status: "success",
      message: "Connection succeeded."
    };
  }
  return invokeCommand<ProviderConnectionTestResponse>("provider_connection_test", { request });
}

export async function listProviderModels(
  request: ProviderModelListRequest
): Promise<ProviderModelListResponse> {
  if (!isTauriRuntime()) {
    return {
      status: "success",
      message: "Model discovery succeeded.",
      models: [
        {
          id: "gpt-4.1-mini",
          display_name: "gpt-4.1-mini",
          context_window: null,
          supports_tools: null
        },
        {
          id: "local-coder",
          display_name: "local-coder",
          context_window: null,
          supports_tools: null
        }
      ]
    };
  }
  return invokeCommand<ProviderModelListResponse>("provider_models_list", { request });
}

export function threadRecordToSession(record: ThreadRecord): SessionSummary {
  return {
    id: record.id,
    projectId: record.project_id,
    title: record.user_title ?? record.fallback_title,
    updatedAt: formatTimestamp(record.updated_at),
    status: mapThreadStatus(record),
    pinned: record.pinned,
    archived: record.archived_at !== null
  };
}

function mapThreadStatus(record: ThreadRecord): SessionStatus {
  if (record.archived_at !== null) {
    return "archived" as const;
  }
  if (record.status === "waiting_approval") {
    return "awaiting_approval" as const;
  }
  if (record.status === "running" || record.status === "idle" || record.status === "failed") {
    return record.status;
  }
  return "idle" as const;
}

function formatTimestamp(value: number) {
  if (value <= 0) {
    return "unknown";
  }
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit"
  }).format(new Date(value));
}

export const exagentClient = {
  archiveThread,
  getWorkbenchSnapshot,
  getProviderSettings,
  getRuntimeSettings,
  listProviderModels,
  testProviderConnection,
  isDesktopRuntime,
  interruptTurn,
  listProjects,
  listThreads,
  pickAndAddProject,
  pinThread,
  readThread,
  reindexProject,
  renameThread,
  replayEvents,
  resumeThread,
  saveProviderSettings,
  saveRuntimeSettings,
  startThread,
  startTurn,
  submitApprovalDecision,
  subscribeRuntimeEvents,
  threadRecordToSession
};
