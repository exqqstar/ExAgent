import type {
  AgentTreeResponse,
  BackendRuntimeEvent,
  ChatGptDeviceCode,
  EventsReplayResponse,
  GitHubCopilotDeviceCode,
  ModelCapabilities,
  ModelRef,
  ProviderConfigView,
  ProviderDescriptor,
  ProviderSettingsResponse,
  ProviderSettingsSaveRequest,
  ProviderConnectionTestRequest,
  ProviderConnectionTestResponse,
  ProviderModelListRequest,
  ProviderModelListResponse,
  ProviderModelView,
  ProjectRecord,
  RuntimeSettingsResponse,
  RuntimeSettingsSaveRequest,
  SessionStatus,
  SessionSummary,
  SkillCatalogScanResponse,
  ThinkingMode,
  ThreadReadResponse,
  ThreadRecord,
  ThreadCompactResponse,
  ThreadGoalClearResponse,
  ThreadGoalGetResponse,
  ThreadGoalSetResponse,
  ThreadGoalStatus,
  ThreadStartResponse,
  TurnInput,
  TurnMode,
  TurnStartResponse,
  WorkbenchSnapshot
} from "@/types";

type Unlisten = () => void;

const mockSnapshot: WorkbenchSnapshot = {
  projects: [
    {
      id: "project-exagent",
      name: "ExAgent",
      path: "/Users/enxiang/dev/ExAgent",
      active: true,
      pinned: false,
      archived: false
    }
  ],
  sessions: [
    {
      id: "session-desktop",
      projectId: "project-exagent",
      title: "Desktop GUI workbench",
      updatedAt: "local preview",
      status: "idle"
    }
  ],
  activeProjectId: "project-exagent",
  activeSessionId: "session-desktop",
  transcript: [
    {
      id: "message-system",
      role: "system",
      title: "Session restored",
      body: "Loaded the desktop workbench thread with sidebar, composer, provider settings, and runtime event history.",
      timestamp: "preview",
      status: "info"
    },
    {
      id: "message-user",
      role: "user",
      body: "Polish the left project navigation so sessions feel nested under the active project instead of living in a separate section.",
      timestamp: "preview"
    },
    {
      id: "message-assistant",
      role: "assistant",
      title: "Updated sidebar interaction",
      body: "I moved sessions into the active project, added a collapsible reveal, tightened the selected session glass card, and kept inactive rows quiet.",
      timestamp: "preview",
      status: "success"
    },
    {
      id: "message-tool-sidebar",
      role: "tool",
      title: "npm test",
      body: "42 tests passed.",
      timestamp: "preview",
      status: "success",
      toolStatus: "completed"
    },
    {
      id: "message-user-empty-state",
      role: "user",
      body: "For a brand-new session, show a focused empty state first. Only create a real session after I send the first message.",
      timestamp: "preview"
    },
    {
      id: "message-assistant-empty-state",
      role: "assistant",
      title: "Draft session behavior",
      body: "Implemented a draft start state with the centered composer. The left session list stays unchanged until the first prompt creates a backend thread.",
      timestamp: "preview",
      status: "info"
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
  cwd: "/Users/enxiang/dev/ExAgent",
  policy: "local",
  tokenUsage: {
    input: 0,
    output: 0,
    limit: 1
  },
  tokenUsageByThreadId: {},
  runtimeSettings: null,
  selectedModel: null,
  selectedThinkingMode: null
};

function mockCapabilities(supportsThinking: boolean, supportsTools = true): ModelCapabilities {
  return {
    supports_tools: supportsTools,
    input_modalities: ["text", "image"],
    thinking: {
      supported: supportsThinking,
      modes: supportsThinking ? ["off", "low", "medium", "high", "x_high"] : []
    }
  };
}

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
      default_model: "gpt-5.5",
      supports_model_discovery: true,
      supports_tools: true,
      unsupported_reason: null
    },
    {
      id: "openai_compatible",
      name: "OpenAI Compatible",
      description: "Use OpenRouter, DeepSeek, local gateways, or another compatible endpoint",
      recommended: false,
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
      description: "Use Claude Pro/Max or an API key",
      recommended: false,
      supported: true,
      auth_mode: "api_key_required",
      protocol: "anthropic_messages",
      default_base_url: "https://api.anthropic.com/v1",
      default_model: "claude-sonnet-4-6",
      supports_model_discovery: false,
      supports_tools: true,
      unsupported_reason: null
    },
    {
      id: "google",
      name: "Google",
      description: "Use Gemini models with a Google API key",
      recommended: false,
      supported: true,
      auth_mode: "api_key_required",
      protocol: "gemini_generate_content",
      default_base_url: "https://generativelanguage.googleapis.com/v1beta",
      default_model: "gemini-3-pro-preview",
      supports_model_discovery: true,
      supports_tools: true,
      unsupported_reason: null
    },
    {
      id: "deepseek",
      name: "DeepSeek",
      description: "Use DeepSeek API with an API key",
      recommended: false,
      supported: true,
      auth_mode: "api_key_required",
      protocol: "openai_chat_completions",
      default_base_url: "https://api.deepseek.com",
      default_model: "deepseek-v4-flash",
      supports_model_discovery: true,
      supports_tools: true,
      unsupported_reason: null
    },
    {
      id: "kimi",
      name: "Kimi",
      description: "Use Kimi API with a Moonshot API key",
      recommended: false,
      supported: true,
      auth_mode: "api_key_required",
      protocol: "openai_chat_completions",
      default_base_url: "https://api.moonshot.ai/v1",
      default_model: "kimi-k2.6",
      supports_model_discovery: true,
      supports_tools: true,
      unsupported_reason: null
    },
    {
      id: "glm",
      name: "GLM",
      description: "Use GLM API with a Zhipu API key",
      recommended: false,
      supported: true,
      auth_mode: "api_key_required",
      protocol: "openai_chat_completions",
      default_base_url: "https://open.bigmodel.cn/api/paas/v4",
      default_model: "glm-5.1",
      supports_model_discovery: true,
      supports_tools: true,
      unsupported_reason: null
    },
    {
      id: "github_copilot",
      name: "GitHub Copilot",
      description: "Use GitHub Copilot with device OAuth",
      recommended: false,
      supported: true,
      auth_mode: "oauth_required",
      protocol: "copilot_oauth",
      default_base_url: "https://api.githubcopilot.com",
      default_model: "gpt-5.5",
      supports_model_discovery: true,
      supports_tools: true,
      unsupported_reason: null
    }
  ],
  active_provider_id: "openai",
  active_credential_id: null,
  credentials: [],
  config: {
    provider_id: "openai",
    base_url: "https://api.openai.com/v1",
    model: "gpt-5.5",
    has_api_key: false,
    has_credential: false,
    credential_kind: null,
    credential_source: "none",
    auth_required: true
  },
  connected_provider: null,
  last_connection: null,
  configured_providers: [],
  model_options: [
    {
      provider_id: "openai",
      id: "gpt-5.5",
      display_name: "gpt-5.5",
      context_window: 1047576,
      supports_tools: true,
      capabilities: mockCapabilities(true)
    }
  ]
};

function mockOAuthProviderSettings(providerId: "openai" | "github_copilot", name: string): ProviderSettingsResponse {
  const provider: ProviderDescriptor = {
    id: providerId,
    name,
    description:
      providerId === "openai" ? "Use ChatGPT Pro/Plus or an API key" : "Use GitHub Copilot with device OAuth",
    recommended: providerId === "openai",
    supported: true,
    auth_mode: providerId === "openai" ? "api_key_required" : "oauth_required",
    protocol: providerId === "openai" ? "openai_chat_completions" : "copilot_oauth",
    default_base_url: providerId === "openai" ? "https://api.openai.com/v1" : "https://api.githubcopilot.com",
    default_model: "gpt-5.5",
    supports_model_discovery: true,
    supports_tools: true,
    unsupported_reason: null
  };
  return {
    ...mockProviderSettings,
    providers: [provider],
    active_provider_id: providerId,
    active_credential_id: providerId === "openai" ? "chatgpt-1" : "copilot-1",
    credentials: [
      {
        id: providerId === "openai" ? "chatgpt-1" : "copilot-1",
        label: providerId === "openai" ? "ChatGPT Pro" : "GitHub Copilot",
        source: "keychain",
        kind: "oauth",
        status: "active",
        auth_method: providerId === "openai" ? "chatgpt_oauth" : "github_copilot_oauth",
        account_label: providerId === "openai" ? "user@example.com" : null
      }
    ],
    config: {
      provider_id: providerId,
      base_url: provider.default_base_url,
      model: provider.default_model,
      has_api_key: false,
      has_credential: true,
      credential_kind: "oauth",
      credential_source: "keychain",
      auth_required: true
    },
    connected_provider: {
      id: providerId,
      name,
      model: provider.default_model,
      base_url: provider.default_base_url
    },
    configured_providers: []
  };
}

let mockRuntimeSettings: RuntimeSettingsResponse = {
  default_model: "gpt-5.5",
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
      model: "gpt-5.5",
      thinking_mode: "high"
    }
  ],
  mcp_servers: [],
  skill_roots: [
    {
      id: "skills-user-preview",
      name: "User skills",
      enabled: true,
      path: "/Users/enxiang/.agents/skills",
      scope: "global"
    }
  ]
};

let mockThreadSequence = 1;

function isTauriRuntime() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export function isDesktopRuntime() {
  return isTauriRuntime();
}

/**
 * Browser-preview only: a sample subagent tree so the Inspector "Agents" panel
 * is visible when the app runs outside the Tauri shell (no live runtime to
 * spawn real agents). Folded through the same reducer the desktop runtime uses.
 */
export function mockSubagentEvents(rootThreadId: string): BackendRuntimeEvent[] {
  const spawn = (
    child: string,
    parent: string,
    task_name: string,
    message_preview: string
  ): BackendRuntimeEvent => ({
    event_id: `mock-spawn-${child}`,
    thread_id: rootThreadId,
    kind: {
      type: "subagent_spawned",
      invocation_id: `mock-inv-${child}`,
      tool_call_id: `mock-call-${child}`,
      parent_thread_id: parent,
      child_thread_id: child,
      task_name,
      message_preview
    }
  });

  return [
    spawn(
      "agent-research",
      rootThreadId,
      "researcher",
      "Map every call site of the inspector store and list the props each panel reads."
    ),
    spawn(
      "agent-scraper",
      "agent-research",
      "scraper",
      "Pull the runtime event kinds from protocol.rs and cross-check the TS union."
    ),
    spawn(
      "agent-tests",
      rootThreadId,
      "test-writer",
      "Add unit tests for the agent tree reducer covering spawn, close, and nesting."
    ),
    {
      event_id: "mock-msg-research",
      thread_id: rootThreadId,
      kind: {
        type: "inter_agent_message_sent",
        invocation_id: "mock-inv-msg",
        tool_call_id: "mock-call-msg",
        author_thread_id: rootThreadId,
        recipient_thread_id: "agent-research",
        author_path: "root",
        recipient_path: "root/researcher",
        content_preview: "Also note which panels read activeSessionId directly.",
        followup: true
      }
    },
    {
      event_id: "mock-close-tests",
      thread_id: rootThreadId,
      kind: {
        type: "subagent_closed",
        invocation_id: "mock-inv-tests",
        tool_call_id: "mock-call-tests",
        parent_thread_id: rootThreadId,
        closed_thread_id: "agent-tests",
        agent_path: "root/test-writer"
      }
    }
  ];
}

export function mockAgentTree(rootThreadId: string): AgentTreeResponse {
  return {
    root: {
      thread_id: rootThreadId,
      root_thread_id: rootThreadId,
      depth: 0,
      agent_path: "root",
      status: "running",
      children: [
        {
          thread_id: "agent-research",
          parent_thread_id: rootThreadId,
          root_thread_id: rootThreadId,
          depth: 1,
          agent_path: "root/researcher",
          status: "running",
          agent_type: "explorer",
          last_task_message: "Map every call site of the inspector store and list the props each panel reads.",
          last_activity: "Also note which panels read activeSessionId directly.",
          children: [
            {
              thread_id: "agent-scraper",
              parent_thread_id: "agent-research",
              root_thread_id: rootThreadId,
              depth: 2,
              agent_path: "root/researcher/scraper",
              status: "running",
              agent_type: "worker",
              last_task_message: "Pull the runtime event kinds from protocol.rs and cross-check the TS union.",
              children: []
            }
          ]
        },
        {
          thread_id: "agent-tests",
          parent_thread_id: rootThreadId,
          root_thread_id: rootThreadId,
          depth: 1,
          agent_path: "root/test-writer",
          status: "done",
          agent_type: "worker",
          last_task_message: "Add unit tests for the agent tree reducer covering spawn, close, and nesting.",
          children: []
        }
      ]
    }
  };
}

async function invokeCommand<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(command, args);
}

async function fallbackSnapshot(): Promise<WorkbenchSnapshot> {
  return mockSnapshot;
}

function mockThreadRead(threadId: string): ThreadReadResponse {
  if (threadId === "session-desktop") {
    return {
      thread: {
        id: threadId,
        status: "idle",
        active_turn: null,
        turns: [
          {
            id: "turn-sidebar-polish",
            status: "completed",
            items: [
              {
                type: "assistant_message",
                event_id: "mock-assistant-sidebar",
                text: "I moved sessions into the active project, added a collapsible reveal, tightened the selected session glass card, and kept inactive rows quiet."
              },
              {
                type: "tool_invocation",
                invocation_id: "mock-tool-tests",
                tool_call_id: "mock-call-tests",
                tool_name: "npm test",
                status: "completed",
                mutating: false,
                output_preview: "42 tests passed."
              }
            ]
          },
          {
            id: "turn-draft-session",
            status: "completed",
            items: [
              {
                type: "assistant_message",
                event_id: "mock-assistant-draft",
                text: "Implemented a draft start state with the centered composer. The left session list stays unchanged until the first prompt creates a backend thread."
              }
            ]
          }
        ]
      }
    };
  }

  return {
    thread: {
      id: threadId,
      status: "idle",
      active_turn: null,
      turns: []
    }
  };
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
      active: project.id === activeProject?.id,
      pinned: project.pinned,
      archived: project.archived_at !== null
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
    tokenUsageByThreadId: {},
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

export async function importImagePaths(paths: string[]): Promise<string[]> {
  if (!isTauriRuntime() || paths.length === 0) {
    return [];
  }
  return invokeCommand<string[]>("image_attachments_import", { paths });
}

export async function pickImageFiles(): Promise<string[]> {
  if (!isTauriRuntime()) {
    return [];
  }

  const { open } = await import("@tauri-apps/plugin-dialog");
  const selected = await open({
    directory: false,
    multiple: true,
    filters: [
      {
        name: "Images",
        extensions: ["png", "jpg", "jpeg", "webp", "gif"]
      }
    ]
  });
  const paths = Array.isArray(selected)
    ? selected.filter((item): item is string => typeof item === "string")
    : typeof selected === "string"
      ? [selected]
      : [];
  if (paths.length === 0) {
    return [];
  }
  return importImagePaths(paths);
}

type ImageAttachmentBytesImport = {
  fileName: string;
  mimeType: string | null;
  bytesBase64: string;
};

const MAX_IMAGE_ATTACHMENT_BYTES = 20 * 1024 * 1024;
const MAX_IMAGE_ATTACHMENT_BATCH_BYTES = 20 * 1024 * 1024;
const MAX_IMAGE_ATTACHMENT_FILE_COUNT = 8;

export async function importImageFiles(files: File[]): Promise<string[]> {
  if (!isTauriRuntime() || files.length === 0) {
    return [];
  }

  validateImageFileBatch(files);

  const items: ImageAttachmentBytesImport[] = [];
  for (const file of files) {
    items.push(await fileToImageAttachmentBytesImport(file));
  }
  return invokeCommand<string[]>("image_attachments_import_bytes", { items });
}

async function fileToImageAttachmentBytesImport(file: File): Promise<ImageAttachmentBytesImport> {
  return {
    fileName: file.name || "image.png",
    mimeType: file.type || null,
    bytesBase64: arrayBufferToBase64(await fileToArrayBuffer(file))
  };
}

function validateImageFileBatch(files: File[]) {
  if (files.length > MAX_IMAGE_ATTACHMENT_FILE_COUNT) {
    throw new Error(
      `Could not import images: ${files.length} files exceeds the ${MAX_IMAGE_ATTACHMENT_FILE_COUNT} file limit`
    );
  }

  let totalBytes = 0;
  for (const file of files) {
    validateImageFileSize(file);
    totalBytes += file.size;
  }

  if (totalBytes > MAX_IMAGE_ATTACHMENT_BATCH_BYTES) {
    throw new Error(
      `Could not import images: ${totalBytes} total bytes exceeds the ${MAX_IMAGE_ATTACHMENT_BATCH_BYTES} byte batch limit`
    );
  }
}

function validateImageFileSize(file: File) {
  const fileName = file.name || "image.png";
  if (file.size === 0) {
    throw new Error(`Could not import image \`${fileName}\`: file is empty`);
  }
  if (file.size > MAX_IMAGE_ATTACHMENT_BYTES) {
    throw new Error(
      `Could not import image \`${fileName}\`: ${file.size} bytes exceeds the ${MAX_IMAGE_ATTACHMENT_BYTES} byte limit`
    );
  }
}

function fileToArrayBuffer(file: File): Promise<ArrayBuffer> {
  if (typeof file.arrayBuffer === "function") {
    return file.arrayBuffer();
  }

  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error("Could not read image file"));
    reader.onload = () => {
      if (reader.result instanceof ArrayBuffer) {
        resolve(reader.result);
      } else {
        reject(new Error("Could not read image file as bytes"));
      }
    };
    reader.readAsArrayBuffer(file);
  });
}

function arrayBufferToBase64(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  let binary = "";
  const chunkSize = 0x8000;
  for (let index = 0; index < bytes.length; index += chunkSize) {
    const chunk = bytes.subarray(index, index + chunkSize);
    binary += String.fromCharCode(...chunk);
  }
  return btoa(binary);
}

export type ImageDragDropHandlers = {
  onEnter: (paths: string[]) => void;
  onLeave: () => void;
  onDrop: (paths: string[]) => void;
};

/**
 * Tauri v2 intercepts OS file drags, so HTML5 drop events do not receive
 * file paths. The webview-level drag-drop event provides those paths.
 */
export async function subscribeImageDragDrop(handlers: ImageDragDropHandlers): Promise<() => void> {
  if (!isTauriRuntime()) {
    return () => undefined;
  }

  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  return getCurrentWebview().onDragDropEvent((event) => {
    if (event.payload.type === "enter") {
      handlers.onEnter(event.payload.paths);
    } else if (event.payload.type === "over") {
      handlers.onEnter([]);
    } else if (event.payload.type === "leave") {
      handlers.onLeave();
    } else if (event.payload.type === "drop") {
      handlers.onDrop(event.payload.paths);
    }
  });
}

export async function listProjects(): Promise<ProjectRecord[]> {
  if (!isTauriRuntime()) {
    return mockSnapshot.projects
      .filter((project) => !project.archived)
      .map(projectSummaryToRecord);
  }
  return invokeCommand<ProjectRecord[]>("project_list");
}

export async function renameProject(projectId: string, name: string): Promise<ProjectRecord> {
  if (!isTauriRuntime()) {
    mockSnapshot.projects = mockSnapshot.projects.map((project) =>
      project.id === projectId ? { ...project, name } : project
    );
    return projectSummaryToRecord(requireMockProject(projectId));
  }
  return invokeCommand<ProjectRecord>("project_rename", { projectId, name });
}

export async function pinProject(projectId: string, pinned: boolean): Promise<ProjectRecord> {
  if (!isTauriRuntime()) {
    mockSnapshot.projects = mockSnapshot.projects.map((project) =>
      project.id === projectId ? { ...project, pinned } : project
    );
    return projectSummaryToRecord(requireMockProject(projectId));
  }
  return invokeCommand<ProjectRecord>("project_pin", { projectId, pinned });
}

export async function archiveProject(projectId: string): Promise<void> {
  if (!isTauriRuntime()) {
    mockSnapshot.projects = mockSnapshot.projects.map((project) =>
      project.id === projectId ? { ...project, archived: true } : project
    );
    return;
  }
  return invokeCommand<void>("project_archive", { projectId });
}

export async function removeProject(projectId: string): Promise<void> {
  if (!isTauriRuntime()) {
    mockSnapshot.projects = mockSnapshot.projects.filter((project) => project.id !== projectId);
    mockSnapshot.sessions = mockSnapshot.sessions.filter((session) => session.projectId !== projectId);
    return;
  }
  return invokeCommand<void>("project_remove", { projectId });
}

export async function archiveProjectConversations(projectId: string): Promise<void> {
  if (!isTauriRuntime()) {
    mockSnapshot.sessions = mockSnapshot.sessions.filter((session) => session.projectId !== projectId);
    return;
  }
  return invokeCommand<void>("project_archive_conversations", { projectId });
}

export async function createProjectWorktree(projectId: string): Promise<ProjectRecord> {
  if (!isTauriRuntime()) {
    const base = requireMockProject(projectId);
    const project = {
      id: `project-worktree-${Date.now()}`,
      name: `${base.name} Worktree`,
      path: `${base.path}-worktree`,
      active: false,
      pinned: false,
      archived: false
    };
    mockSnapshot.projects = [project, ...mockSnapshot.projects];
    return projectSummaryToRecord(project);
  }
  return invokeCommand<ProjectRecord>("project_create_worktree", { projectId });
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

export async function revealProjectInFileManager(path: string): Promise<void> {
  if (!isTauriRuntime()) {
    return;
  }

  return invokeCommand<void>("project_reveal_in_file_manager", { path });
}

export async function startThread(projectId: string): Promise<ThreadStartResponse> {
  if (!isTauriRuntime()) {
    const now = Date.now();
    const id = `session-mock-${mockThreadSequence++}`;
    mockSnapshot.sessions = [
      {
        id,
        projectId,
        title: "New session",
        updatedAt: new Intl.DateTimeFormat(undefined, {
          month: "short",
          day: "numeric",
          hour: "2-digit",
          minute: "2-digit"
        }).format(new Date(now)),
        status: "idle"
      },
      ...mockSnapshot.sessions
    ];
    return {
      thread: {
        id,
        status: "idle",
        active_turn: null,
        turns: []
      }
    };
  }

  return invokeCommand<ThreadStartResponse>("thread_start", { projectId });
}

export async function readThread(projectId: string, threadId: string): Promise<ThreadReadResponse> {
  if (!isTauriRuntime()) {
    return mockThreadRead(threadId);
  }

  return invokeCommand<ThreadReadResponse>("thread_read", { projectId, threadId });
}

export async function resumeThread(projectId: string, threadId: string): Promise<ThreadReadResponse> {
  if (!isTauriRuntime()) {
    return mockThreadRead(threadId);
  }

  return invokeCommand<ThreadReadResponse>("thread_resume", { projectId, threadId });
}

export async function compactThread(projectId: string, threadId: string): Promise<ThreadCompactResponse> {
  if (!isTauriRuntime()) {
    return {
      thread_id: threadId,
      latest_compaction: null
    };
  }

  return invokeCommand<ThreadCompactResponse>("thread_compact", { projectId, threadId });
}

export async function setThreadGoal(
  projectId: string,
  threadId: string,
  input: {
    objective?: string | null;
    status?: ThreadGoalStatus | null;
    tokenBudget?: number | null;
    clearTokenBudget?: boolean;
  }
): Promise<ThreadGoalSetResponse> {
  if (!isTauriRuntime()) {
    throw new Error("Goal mode is available in the desktop runtime.");
  }

  const tokenBudget = input.clearTokenBudget ? null : input.tokenBudget;
  return invokeCommand<ThreadGoalSetResponse>("thread_goal_set", {
    projectId,
    threadId,
    objective: input.objective ?? null,
    status: input.status ?? null,
    tokenBudget: input.clearTokenBudget || input.tokenBudget !== undefined ? tokenBudget : undefined,
    clearTokenBudget: input.clearTokenBudget || undefined
  });
}

export async function getThreadGoal(projectId: string, threadId: string): Promise<ThreadGoalGetResponse> {
  if (!isTauriRuntime()) {
    return { goal: null };
  }

  return invokeCommand<ThreadGoalGetResponse>("thread_goal_get", { projectId, threadId });
}

export async function clearThreadGoal(projectId: string, threadId: string): Promise<ThreadGoalClearResponse> {
  if (!isTauriRuntime()) {
    return { cleared: false };
  }

  return invokeCommand<ThreadGoalClearResponse>("thread_goal_clear", { projectId, threadId });
}

export async function agentTree(projectId: string, threadId: string): Promise<AgentTreeResponse> {
  if (!isTauriRuntime()) {
    return mockAgentTree(threadId);
  }

  return invokeCommand<AgentTreeResponse>("agent_tree", { projectId, threadId });
}

export type StartTurnOptions = {
  model?: ModelRef | null;
  thinkingMode?: ThinkingMode | null;
  clearThinkingMode?: boolean;
  turnMode?: TurnMode;
  input?: TurnInput[];
};

export async function startTurn(
  projectId: string,
  threadId: string,
  prompt: string,
  options: StartTurnOptions = {}
): Promise<TurnStartResponse> {
  const model = options.model ?? null;
  const thinkingMode = options.thinkingMode ?? null;
  const clearThinkingMode = options.clearThinkingMode ?? false;
  const turnMode = options.turnMode ?? "default";
  const input = options.input ?? [];

  if (!isTauriRuntime()) {
    return {
      thread_id: threadId,
      turn: {
        id: `turn-${Date.now()}`,
        status: "in_progress",
        items: [
          {
            type: "user_message",
            text: prompt,
            input
          }
        ]
      }
    };
  }

  const args: Record<string, unknown> = {
    projectId,
    threadId,
    prompt,
    model: model?.provider_id.trim() && model?.model_id.trim() ? model : null,
    thinkingMode: thinkingMode ?? null,
    clearThinkingMode,
    turnMode
  };
  if (input.length > 0) {
    args.input = input;
  }
  return invokeCommand<TurnStartResponse>("turn_start", args);
}

export async function interruptTurn(projectId: string, threadId: string, turnId?: string) {
  return invokeCommand("turn_interrupt", { projectId, threadId, turnId: turnId ?? null });
}

export async function renameThread(threadId: string, title: string) {
  if (!isTauriRuntime()) {
    mockSnapshot.sessions = mockSnapshot.sessions.map((session) =>
      session.id === threadId ? { ...session, title } : session
    );
    return;
  }
  return invokeCommand("thread_rename", { threadId, title });
}

export async function pinThread(threadId: string, pinned: boolean) {
  if (!isTauriRuntime()) {
    mockSnapshot.sessions = mockSnapshot.sessions.map((session) =>
      session.id === threadId ? { ...session, pinned } : session
    );
    return;
  }
  return invokeCommand("thread_pin", { threadId, pinned });
}

export async function archiveThread(threadId: string) {
  if (!isTauriRuntime()) {
    mockSnapshot.sessions = mockSnapshot.sessions.filter((session) => session.id !== threadId);
    return;
  }
  return invokeCommand("thread_archive", { threadId });
}

export async function unarchiveThread(threadId: string) {
  if (!isTauriRuntime()) {
    return;
  }
  return invokeCommand("thread_unarchive", { threadId });
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
  if (!isTauriRuntime()) {
    return {
      thread_id: threadId,
      events: []
    };
  }

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
  try {
    await invokeCommand("events_subscribe", { projectId, threadId, afterEventId: null });
  } catch (error) {
    unlisten();
    throw error;
  }
  return () => {
    void invokeCommand("events_unsubscribe", { projectId, threadId }).catch(() => undefined);
    unlisten();
  };
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

export async function scanSkillCatalog(workspaceRoot?: string | null): Promise<SkillCatalogScanResponse> {
  if (!isTauriRuntime()) {
    return mockSkillCatalogScan(workspaceRoot ?? mockSnapshot.cwd);
  }
  return invokeCommand<SkillCatalogScanResponse>("skill_catalog_scan", { workspaceRoot: workspaceRoot ?? null });
}

export async function saveRuntimeSettings(
  request: RuntimeSettingsSaveRequest
): Promise<RuntimeSettingsResponse> {
  if (!isTauriRuntime()) {
    mockRuntimeSettings = request;
    return mockRuntimeSettings;
  }
  return invokeCommand<RuntimeSettingsResponse>("runtime_settings_save", { request });
}

export async function saveProviderSettings(
  request: ProviderSettingsSaveRequest
): Promise<ProviderSettingsResponse> {
  if (!isTauriRuntime()) {
    const provider = mockProviderSettings.providers.find((item) => item.id === request.providerId);
    const authRequired = provider?.auth_mode === "api_key_required";
    const hasApiKey = Boolean(request.apiKey && !request.clearApiKey) || provider?.auth_mode === "api_key_optional";
    const config: ProviderConfigView = {
      provider_id: request.providerId,
      base_url: request.baseUrl,
      model: request.model,
      has_api_key: hasApiKey,
      has_credential: hasApiKey,
      credential_kind: hasApiKey ? ("api_key" as const) : null,
      credential_source: hasApiKey && provider?.auth_mode !== "api_key_optional" ? ("keychain" as const) : ("none" as const),
      auth_required: authRequired
    };
    const configuredProviders = [
      ...(mockProviderSettings.configured_providers ?? []).filter((item) => item.provider_id !== request.providerId),
      ...(hasApiKey || !authRequired ? [config] : [])
    ];
    const nextModelOptions = mergeProviderModelOptions(
      mockProviderSettings.model_options,
      request.providerId,
      request.modelOptions?.length
        ? request.modelOptions
        : [
            {
              provider_id: request.providerId,
              id: request.model,
              display_name: request.model,
              context_window: null,
              supports_tools: provider?.supports_tools ?? null,
              capabilities: mockCapabilities(false, provider?.supports_tools ?? false)
            }
          ]
    );
    return {
      ...mockProviderSettings,
      active_provider_id: request.providerId,
      active_credential_id: hasApiKey ? "key-1" : null,
      credentials: hasApiKey
        ? [
            {
              id: "key-1",
              label: "API key 1",
              source: "keychain" as const,
              kind: "api_key" as const,
              status: "active" as const,
              auth_method: null,
              account_label: null
            }
          ]
        : [],
      config,
      connected_provider:
        hasApiKey || !authRequired
          ? {
              id: request.providerId,
              name: provider?.name ?? request.providerId,
              model: request.model,
              base_url: request.baseUrl
            }
          : null,
      last_connection: null,
      configured_providers: configuredProviders,
      model_options: nextModelOptions
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
          provider_id: request.providerId,
          id: "gpt-4.1-mini",
          display_name: "gpt-4.1-mini",
          context_window: null,
          supports_tools: null,
          capabilities: mockCapabilities(false)
        },
        {
          provider_id: request.providerId,
          id: "local-coder",
          display_name: "local-coder",
          context_window: null,
          supports_tools: null,
          capabilities: mockCapabilities(false)
        }
      ]
    };
  }
  return invokeCommand<ProviderModelListResponse>("provider_models_list", { request });
}

export async function startChatGptOAuthDevice(): Promise<ChatGptDeviceCode> {
  if (!isTauriRuntime()) {
    return {
      device_auth_id: "mock-chatgpt-device",
      user_code: "CHAT-GPT",
      verification_uri: "https://auth.openai.com/codex/device",
      expires_in: 900,
      interval: 1
    };
  }
  return invokeCommand<ChatGptDeviceCode>("provider_chatgpt_oauth_device_start");
}

export async function completeChatGptOAuthDevice(
  device: ChatGptDeviceCode
): Promise<ProviderSettingsResponse> {
  if (!isTauriRuntime()) {
    return mockOAuthProviderSettings("openai", "OpenAI");
  }
  return invokeCommand<ProviderSettingsResponse>("provider_chatgpt_oauth_device_complete", { device });
}

export async function startGitHubCopilotOAuthDevice(): Promise<GitHubCopilotDeviceCode> {
  if (!isTauriRuntime()) {
    return {
      device_code: "mock-copilot-device",
      user_code: "COPI-LOT",
      verification_uri: "https://github.com/login/device",
      expires_in: 900,
      interval: 1
    };
  }
  return invokeCommand<GitHubCopilotDeviceCode>("provider_github_copilot_oauth_device_start");
}

export async function completeGitHubCopilotOAuthDevice(
  device: GitHubCopilotDeviceCode
): Promise<ProviderSettingsResponse> {
  if (!isTauriRuntime()) {
    return mockOAuthProviderSettings("github_copilot", "GitHub Copilot");
  }
  return invokeCommand<ProviderSettingsResponse>("provider_github_copilot_oauth_device_complete", { device });
}

export async function openExternalUrl(url: string): Promise<void> {
  if (!isTauriRuntime()) {
    window.open(url, "_blank", "noopener,noreferrer");
    return;
  }
  return invokeCommand<void>("open_external_url", { url });
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

function requireMockProject(projectId: string) {
  const project = mockSnapshot.projects.find((item) => item.id === projectId);
  if (!project) {
    throw new Error("Project not found");
  }
  return project;
}

function projectSummaryToRecord(project: {
  id: string;
  name: string;
  path: string;
  archived?: boolean;
  pinned?: boolean;
}): ProjectRecord {
  return {
    id: project.id,
    name: project.name,
    path: project.path,
    archived_at: project.archived ? Date.now() : null,
    pinned: project.pinned ?? false
  };
}

function mergeProviderModelOptions(
  existing: ProviderModelView[],
  providerId: string,
  models: ProviderModelView[]
) {
  const retained = existing.filter((model) => model.provider_id !== providerId);
  const merged = [...retained];
  const seen = new Set(merged.map((model) => `${model.provider_id}:${model.id}`));
  models.forEach((model) => {
    const key = `${model.provider_id}:${model.id}`;
    if (!seen.has(key)) {
      merged.push(model);
      seen.add(key);
    }
  });
  return merged;
}

function mockSkillCatalogScan(workspaceRoot: string | null): SkillCatalogScanResponse {
  const projectRoot = workspaceRoot ? `${workspaceRoot}/.agents/skills` : "";
  const globalRoot = mockRuntimeSettings.skill_roots[0] ?? {
    id: "skills-user-preview",
    name: "User skills",
    enabled: true,
    path: "/Users/enxiang/.agents/skills",
    scope: "global"
  };

  return {
    sources: [
      {
        id: "project",
        name: "Project skills",
        scope: "project",
        enabled: true,
        path: projectRoot,
        status: "ready",
        skill_count: 1,
        warning_count: 0
      },
      {
        id: globalRoot.id,
        name: globalRoot.name || "User skills",
        scope: globalRoot.scope || "global",
        enabled: globalRoot.enabled,
        path: globalRoot.path,
        status: globalRoot.enabled ? "ready" : "disabled",
        skill_count: 2,
        warning_count: 0
      }
    ],
    skills: [
      {
        name: "project-memory",
        scope: "project",
        description: "Summarize project-local conventions before a coding turn.",
        path: `${projectRoot}/project-memory/SKILL.md`,
        source_id: "project",
        allow_implicit_invocation: true,
        effective_implicit: true,
        status: "active"
      },
      {
        name: "release-notes",
        scope: globalRoot.scope || "global",
        description: "Draft concise release notes from changed files.",
        path: `${globalRoot.path}/release-notes/SKILL.md`,
        source_id: globalRoot.id,
        allow_implicit_invocation: true,
        effective_implicit: true,
        status: "active"
      },
      {
        name: "billing-audit",
        scope: globalRoot.scope || "global",
        description: "Review billing-sensitive code paths when explicitly requested.",
        path: `${globalRoot.path}/billing-audit/SKILL.md`,
        source_id: globalRoot.id,
        allow_implicit_invocation: false,
        effective_implicit: false,
        status: "explicit_only"
      }
    ],
    warnings: []
  };
}

export const exagentClient = {
  archiveProject,
  archiveProjectConversations,
  archiveThread,
  createProjectWorktree,
  getWorkbenchSnapshot,
  getProviderSettings,
  getRuntimeSettings,
  scanSkillCatalog,
  listProviderModels,
  startChatGptOAuthDevice,
  completeChatGptOAuthDevice,
  startGitHubCopilotOAuthDevice,
  completeGitHubCopilotOAuthDevice,
  openExternalUrl,
  testProviderConnection,
  isDesktopRuntime,
  mockSubagentEvents,
  mockAgentTree,
  interruptTurn,
  importImageFiles,
  importImagePaths,
  agentTree,
  listProjects,
  listThreads,
  pickAndAddProject,
  pickImageFiles,
  pinProject,
  pinThread,
  readThread,
  removeProject,
  revealProjectInFileManager,
  reindexProject,
  renameProject,
  renameThread,
  compactThread,
  replayEvents,
  resumeThread,
  saveProviderSettings,
  saveRuntimeSettings,
  setThreadGoal,
  startThread,
  startTurn,
  submitApprovalDecision,
  subscribeImageDragDrop,
  subscribeRuntimeEvents,
  threadRecordToSession,
  clearThreadGoal,
  getThreadGoal,
  unarchiveThread
};
