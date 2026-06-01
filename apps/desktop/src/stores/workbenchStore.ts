import { create } from "zustand";
import { exagentClient } from "@/api/exagentClient";
import type {
  BackendRuntimeEvent,
  ModelRef,
  ProjectSummary,
  RuntimeEvent,
  RuntimeSettingsSaveRequest,
  SessionSummary,
  ThinkingMode,
  ThreadItem,
  ThreadView,
  TranscriptMessage,
  WorkbenchSnapshot
} from "@/types";

type Unlisten = () => void;
const DEFAULT_PROVIDER_ID = "openai";

type WorkbenchState = WorkbenchSnapshot & {
  loading: boolean;
  error: string | null;
  composerValue: string;
  search: string;
  activeProviderId: string | null;
  eventUnlisten: Unlisten | null;
  loadWorkbench: () => Promise<void>;
  addProject: () => Promise<void>;
  selectProject: (projectId: string) => Promise<void>;
  openSession: (sessionId: string) => Promise<void>;
  startSession: () => Promise<string | null>;
  sendPrompt: () => Promise<void>;
  interruptActiveTurn: () => Promise<void>;
  submitApproval: (message: TranscriptMessage, decision: "approved" | "denied") => Promise<void>;
  renameSession: (sessionId: string, title: string) => Promise<void>;
  archiveSession: (sessionId: string) => Promise<void>;
  pinSession: (sessionId: string, pinned: boolean) => Promise<void>;
  setComposerValue: (composerValue: string) => void;
  setSelectedModel: (model: string | ModelRef | null) => void;
  setSelectedThinkingMode: (thinkingMode: ThinkingMode | null) => void;
  applyRuntimePreset: (presetId: string) => void;
  saveRuntimeSettings: (settings: RuntimeSettingsSaveRequest) => Promise<void>;
  setSearch: (search: string) => Promise<void>;
  applyRuntimeEvent: (event: BackendRuntimeEvent) => void;
};

const emptySnapshot: WorkbenchSnapshot = {
  projects: [],
  sessions: [],
  activeProjectId: null,
  activeSessionId: null,
  transcript: [],
  events: [],
  changedFiles: [],
  cwd: "No project selected",
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

export const useWorkbenchStore = create<WorkbenchState>((set, get) => ({
  ...emptySnapshot,
  loading: true,
  error: null,
  composerValue: "",
  search: "",
  activeProviderId: DEFAULT_PROVIDER_ID,
  eventUnlisten: null,

  async loadWorkbench() {
    set({ loading: true, error: null });
    try {
      const snapshot = await exagentClient.getWorkbenchSnapshot();
      const [runtimeSettings, providerSettings] = await Promise.all([
        exagentClient.getRuntimeSettings(),
        exagentClient.getProviderSettings()
      ]);
      const activeProviderId = providerSettings.active_provider_id || DEFAULT_PROVIDER_ID;
      set({
        ...snapshot,
        runtimeSettings,
        selectedModel: modelRefFromString(runtimeSettings.default_model, activeProviderId),
        selectedThinkingMode: runtimeSettings.default_thinking_mode,
        activeProviderId,
        loading: false,
        error: null
      });
      if (snapshot.activeSessionId && exagentClient.isDesktopRuntime()) {
        await get().openSession(snapshot.activeSessionId);
      }
    } catch (error) {
      set({
        ...emptySnapshot,
        activeProviderId: DEFAULT_PROVIDER_ID,
        loading: false,
        error: errorMessage(error)
      });
    }
  },

  async addProject() {
    set({ error: null });
    try {
      const project = await exagentClient.pickAndAddProject();
      if (!project) {
        return;
      }
      const projects = await exagentClient.listProjects();
      const sessions = get().search
        ? await exagentClient.listThreads(project.id, false, get().search)
        : await exagentClient.reindexProject(project.id);
      set({
        projects: projects.map((item) => projectRecordToSummary(item, item.id === project.id)),
        sessions: sessions.map(exagentClient.threadRecordToSession),
        activeProjectId: project.id,
        activeSessionId: sessions[0]?.id ?? null,
        transcript: [],
        events: [],
        cwd: project.path
      });
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async selectProject(projectId: string) {
    const project = get().projects.find((item) => item.id === projectId);
    if (!project) {
      return;
    }
    set({ loading: true, error: null });
    try {
      const threads = get().search
        ? await exagentClient.listThreads(projectId, false, get().search)
        : await exagentClient.reindexProject(projectId);
      set({
        projects: get().projects.map((item) => ({ ...item, active: item.id === projectId })),
        sessions: threads.map(exagentClient.threadRecordToSession),
        activeProjectId: projectId,
        activeSessionId: threads[0]?.id ?? null,
        transcript: [],
        events: [],
        changedFiles: [],
        cwd: project.path,
        loading: false
      });
      if (threads[0]) {
        await get().openSession(threads[0].id);
      }
    } catch (error) {
      set({ loading: false, error: errorMessage(error) });
    }
  },

  async openSession(sessionId: string) {
    const projectId = get().activeProjectId;
    if (!projectId) {
      return;
    }
    set({ activeSessionId: sessionId, error: null });
    try {
      get().eventUnlisten?.();
      const read = await exagentClient.resumeThread(projectId, sessionId);
      const replay = await exagentClient.replayEvents(projectId, sessionId, null);
      const unlisten = await exagentClient.subscribeRuntimeEvents(projectId, sessionId, (event) => {
        if (event.thread_id === get().activeSessionId) {
          get().applyRuntimeEvent(event);
        }
      });
      set({
        transcript: threadViewToTranscript(read.thread),
        events: replay.events.map(runtimeEventToInspector),
        eventUnlisten: unlisten
      });
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async startSession() {
    const projectId = get().activeProjectId;
    if (!projectId) {
      return null;
    }
    try {
      const started = await exagentClient.startThread(projectId);
      const threads = await exagentClient.listThreads(projectId, false, get().search || null);
      set({
        sessions: threads.map(exagentClient.threadRecordToSession),
        activeSessionId: started.thread.id,
        transcript: [],
        events: []
      });
      await get().openSession(started.thread.id);
      return started.thread.id;
    } catch (error) {
      set({ error: errorMessage(error) });
      return null;
    }
  },

  async sendPrompt() {
    const prompt = get().composerValue.trim();
    if (!prompt) {
      return;
    }
    const projectId = get().activeProjectId;
    if (!projectId) {
      set({ error: "Choose a project folder first." });
      return;
    }
    let threadId = get().activeSessionId;
    if (!threadId) {
      threadId = await get().startSession();
    }
    if (!threadId) {
      return;
    }

    const optimisticMessage: TranscriptMessage = {
      id: `user-${Date.now()}`,
      role: "user",
      body: prompt,
      timestamp: "now",
      threadId
    };
    set({
      composerValue: "",
      transcript: [...get().transcript, optimisticMessage],
      sessions: get().sessions.map((session) =>
        session.id === threadId ? { ...session, status: "running" } : session
      )
    });

    try {
      await exagentClient.startTurn(
        projectId,
        threadId,
        prompt,
        get().selectedModel ?? modelRefFromString(get().runtimeSettings?.default_model, get().activeProviderId),
        get().selectedThinkingMode ?? get().runtimeSettings?.default_thinking_mode ?? null
      );
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async interruptActiveTurn() {
    const projectId = get().activeProjectId;
    const threadId = get().activeSessionId;
    if (!projectId || !threadId) {
      return;
    }
    try {
      await exagentClient.interruptTurn(projectId, threadId);
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async submitApproval(message: TranscriptMessage, decision: "approved" | "denied") {
    const projectId = get().activeProjectId;
    const threadId = message.threadId ?? get().activeSessionId;
    if (!projectId || !threadId || !message.approvalId) {
      return;
    }
    try {
      await exagentClient.submitApprovalDecision(
        projectId,
        threadId,
        message.turnId,
        message.approvalId,
        decision,
        decision === "approved" ? "desktop approved" : "desktop denied"
      );
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async renameSession(sessionId: string, title: string) {
    const nextTitle = title.trim();
    if (!nextTitle) {
      return;
    }
    try {
      await exagentClient.renameThread(sessionId, nextTitle);
      set({
        sessions: get().sessions.map((session) =>
          session.id === sessionId ? { ...session, title: nextTitle } : session
        )
      });
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async archiveSession(sessionId: string) {
    try {
      await exagentClient.archiveThread(sessionId);
      set({
        sessions: get().sessions.filter((session) => session.id !== sessionId),
        activeSessionId: get().activeSessionId === sessionId ? null : get().activeSessionId
      });
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async pinSession(sessionId: string, pinned: boolean) {
    try {
      await exagentClient.pinThread(sessionId, pinned);
      set({
        sessions: get().sessions.map((session) =>
          session.id === sessionId ? { ...session, pinned } : session
        )
      });
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  setComposerValue(composerValue: string) {
    set({ composerValue });
  },

  setSelectedModel(model) {
    set({ selectedModel: normalizeModelRef(model, get().activeProviderId) });
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
      selectedModel: modelRefFromString(preset.model, get().activeProviderId),
      selectedThinkingMode: preset.thinking_mode
    });
  },

  async saveRuntimeSettings(settings) {
    try {
      const runtimeSettings = await exagentClient.saveRuntimeSettings(settings);
      set({
        runtimeSettings,
        selectedModel: modelRefFromString(runtimeSettings.default_model, get().activeProviderId),
        selectedThinkingMode: runtimeSettings.default_thinking_mode,
        error: null
      });
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async setSearch(search: string) {
    set({ search });
    const projectId = get().activeProjectId;
    if (!projectId) {
      return;
    }
    const threads = await exagentClient.listThreads(projectId, false, search || null);
    set({ sessions: threads.map(exagentClient.threadRecordToSession) });
  },

  applyRuntimeEvent(event: BackendRuntimeEvent) {
    const current = get();
    const transcriptMessage = runtimeEventToTranscript(event);
    const inspectorEvent = runtimeEventToInspector(event);
    const activeThreadId = current.activeSessionId;
    const nextSessions = current.sessions.map((session) => {
      if (session.id !== event.thread_id) {
        return session;
      }
      return { ...session, status: statusFromEvent(event, session.status) };
    });

    set({
      transcript:
        transcriptMessage && event.thread_id === activeThreadId
          ? [...current.transcript, transcriptMessage]
          : current.transcript,
      events: [inspectorEvent, ...current.events].slice(0, 80),
      sessions: nextSessions
    });
  }
}));

export function getWorkbenchState() {
  return useWorkbenchStore.getState();
}

export const loadWorkbench = () => useWorkbenchStore.getState().loadWorkbench();
export const setComposerValue = (composerValue: string) =>
  useWorkbenchStore.getState().setComposerValue(composerValue);
export const setSelectedModel = (model: string | ModelRef | null) =>
  useWorkbenchStore.getState().setSelectedModel(model);
export const setSelectedThinkingMode = (thinkingMode: ThinkingMode | null) =>
  useWorkbenchStore.getState().setSelectedThinkingMode(thinkingMode);
export const applyRuntimePreset = (presetId: string) =>
  useWorkbenchStore.getState().applyRuntimePreset(presetId);
export const sendPrompt = () => useWorkbenchStore.getState().sendPrompt();
export const interruptActiveTurn = () => useWorkbenchStore.getState().interruptActiveTurn();
export const submitApproval = (message: TranscriptMessage, decision: "approved" | "denied") =>
  useWorkbenchStore.getState().submitApproval(message, decision);

function normalizeModelRef(model: string | ModelRef | null, providerId?: string | null): ModelRef | null {
  if (typeof model === "string" || model === null) {
    return modelRefFromString(model, providerId);
  }

  const normalizedProviderId = model.provider_id.trim();
  const normalizedModelId = model.model_id.trim();
  if (!normalizedProviderId || !normalizedModelId) {
    return null;
  }
  return {
    provider_id: normalizedProviderId,
    model_id: normalizedModelId
  };
}

function modelRefFromString(model: string | null | undefined, providerId?: string | null): ModelRef | null {
  const modelId = model?.trim();
  if (!modelId) {
    return null;
  }
  return {
    provider_id: providerId?.trim() || DEFAULT_PROVIDER_ID,
    model_id: modelId
  };
}

function projectRecordToSummary(project: { id: string; name: string; path: string }, active: boolean): ProjectSummary {
  return {
    id: project.id,
    name: project.name,
    path: project.path,
    active
  };
}

function threadViewToTranscript(thread: ThreadView): TranscriptMessage[] {
  return thread.turns.flatMap((turn) =>
    turn.items
      .map((item, index) => threadItemToTranscript(thread.id, turn.id, item, index))
      .filter((item): item is TranscriptMessage => item !== null)
  );
}

function threadItemToTranscript(
  threadId: string,
  turnId: string,
  item: ThreadItem,
  index: number
): TranscriptMessage | null {
  const id = `${threadId}-${turnId}-${item.type}-${index}`;
  switch (item.type) {
    case "assistant_message":
      return {
        id,
        role: "assistant",
        body: item.text ?? "",
        timestamp: "history",
        threadId,
        turnId
      };
    case "tool_result":
      return {
        id,
        role: "tool",
        title: item.name,
        body: "Tool completed.",
        timestamp: "history",
        status: "info",
        threadId,
        turnId
      };
    case "exec_output":
      return {
        id,
        role: "tool",
        title: "Command output",
        body: item.text,
        timestamp: "history",
        status: "info",
        threadId,
        turnId
      };
    case "approval_requested":
      return {
        id,
        role: "approval",
        title: "Approval requested",
        body: item.reason,
        timestamp: "history",
        status: "warning",
        threadId,
        turnId,
        approvalId: item.approval_id,
        toolName: item.tool_name
      };
    case "approval_decision":
      return {
        id,
        role: "tool",
        title: `Approval ${item.status}`,
        body: item.note ?? item.status,
        timestamp: "history",
        status: item.status === "approved" ? "success" : "danger",
        threadId,
        turnId
      };
    case "runtime_error":
      return {
        id,
        role: "system",
        title: "Runtime error",
        body: item.message,
        timestamp: "history",
        status: "danger",
        threadId,
        turnId
      };
    case "compaction_written":
    case "user_message":
      return null;
  }
}

function runtimeEventToTranscript(event: BackendRuntimeEvent): TranscriptMessage | null {
  const base = {
    id: event.event_id,
    timestamp: "now",
    threadId: event.thread_id,
    turnId: event.turn_id ?? undefined
  };

  switch (event.kind.type) {
    case "assistant_turn":
      if (!event.kind.turn.text) {
        return null;
      }
      return {
        ...base,
        role: "assistant",
        body: event.kind.turn.text
      };
    case "tool_result":
      return {
        ...base,
        role: "tool",
        title: event.kind.result.tool_name,
        body: event.kind.result.content,
        status: event.kind.result.status === "success" ? "success" : "info"
      };
    case "exec_output":
      return {
        ...base,
        role: "tool",
        title: event.kind.stream,
        body: event.kind.chunk,
        status: "info"
      };
    case "approval_requested":
      return {
        ...base,
        role: "approval",
        title: "Approval requested",
        body: event.kind.reason,
        status: "warning",
        approvalId: event.kind.approval_id,
        toolName: event.kind.tool_name
      };
    case "approval_decision":
      return {
        ...base,
        role: "tool",
        title: `Approval ${event.kind.status}`,
        body: event.kind.note ?? event.kind.status,
        status: event.kind.status === "approved" ? "success" : "danger"
      };
    case "runtime_error":
      return {
        ...base,
        role: "system",
        title: "Runtime error",
        body: event.kind.message,
        status: "danger"
      };
    default:
      return null;
  }
}

function runtimeEventToInspector(event: BackendRuntimeEvent): RuntimeEvent {
  return {
    id: event.event_id,
    label: event.kind.type.replaceAll("_", " "),
    detail: eventDetail(event),
    timestamp: "now",
    tone: eventTone(event)
  };
}

function eventDetail(event: BackendRuntimeEvent) {
  switch (event.kind.type) {
    case "assistant_turn":
      return event.kind.turn.text ?? "Assistant turn";
    case "tool_result":
      return event.kind.result.tool_name;
    case "approval_requested":
      return event.kind.reason;
    case "approval_decision":
      return event.kind.note ?? event.kind.status;
    case "runtime_error":
      return event.kind.message;
    default:
      return event.thread_id;
  }
}

function eventTone(event: BackendRuntimeEvent): RuntimeEvent["tone"] {
  switch (event.kind.type) {
    case "runtime_error":
      return "danger";
    case "approval_requested":
      return "warning";
    case "approval_decision":
      return event.kind.status === "approved" ? "success" : "danger";
    case "turn_completed":
      return "success";
    default:
      return "info";
  }
}

function statusFromEvent(event: BackendRuntimeEvent, current: SessionSummary["status"]) {
  switch (event.kind.type) {
    case "turn_started":
      return "running";
    case "approval_requested":
      return "awaiting_approval";
    case "runtime_error":
      return "failed";
    case "turn_completed":
    case "turn_interrupted":
      return "idle";
    default:
      return current;
  }
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}
