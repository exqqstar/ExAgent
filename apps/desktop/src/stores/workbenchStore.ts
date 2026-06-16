import { create } from "zustand";
import { exagentClient } from "@/api/exagentClient";
import {
  agentForestFromTreeResponse,
  agentRecordsFromThreadView,
  applyAgentEvent,
  buildAgentForest,
  flattenAgentForest
} from "@/lib/agentTree";
import type {
  AgentNode,
  AgentRunStatus,
  AgentThreadView,
  ApprovalActionStatus,
  BackendRuntimeEvent,
  ComposerAttachment,
  DraftThreadGoal,
  InputModality,
  ModelRef,
  PendingApprovalItem,
  ProviderSettingsResponse,
  ProjectSummary,
  RuntimeEvent,
  RuntimeSettingsSaveRequest,
  SessionSummary,
  ThreadGoal,
  ThreadGoalMode,
  ThreadGoalReport,
  ThreadGoalStatus,
  ThinkingMode,
  TurnInput,
  ThreadItem,
  ThreadTokenUsage,
  ThreadView,
  ToolInvocationTranscriptStatus,
  TranscriptMessage,
  WorkbenchSnapshot
} from "@/types";

type Unlisten = () => void;
const DEFAULT_PROVIDER_ID = "openai";
const AGENT_TREE_REFRESH_DEBOUNCE_MS = 300;
const APPROVALS_REFRESH_DEBOUNCE_MS = 300;
let openSessionRequestSequence = 0;
let openAgentThreadRequestSequence = 0;
type AgentTreeRefreshContext = { projectId: string; threadId: string; sessionGeneration: number };
let agentTreeRefreshTimeoutId: ReturnType<typeof setTimeout> | null = null;
let pendingAgentTreeRefreshContext: AgentTreeRefreshContext | null = null;
type ApprovalsRefreshContext = { projectId: string; sessionGeneration: number };
let approvalsRefreshTimeoutId: ReturnType<typeof setTimeout> | null = null;
let pendingApprovalsRefreshContext: ApprovalsRefreshContext | null = null;
let branchCompareRequestSequence = 0;

type ApprovalsStatus = "idle" | "loading" | "ready" | "error" | "submitting";

type BranchCompareView = {
  parentThreadId: string;
  childThreadId: string;
  parentTitle: string;
  childTitle: string;
  parentTranscript: TranscriptMessage[];
  childTranscript: TranscriptMessage[];
  sharedTurnCount: number;
  forkPointTurnId: string;
  loading: boolean;
  error: string | null;
};

type ThreadSelection = {
  selectedModel: ModelRef | null;
  selectedThinkingMode: ThinkingMode | null;
};

type WorkbenchState = WorkbenchSnapshot & {
  loading: boolean;
  error: string | null;
  agents: AgentNode[];
  composerValue: string;
  composerAttachments: ComposerAttachment[];
  currentGoal: ThreadGoal | null;
  currentGoalMode: ThreadGoalMode;
  draftGoal: DraftThreadGoal | null;
  goalEditorOpen: boolean;
  search: string;
  activeProviderId: string | null;
  providerSettings: ProviderSettingsResponse | null;
  selectionByThreadId: Record<string, ThreadSelection>;
  eventUnlisten: Unlisten | null;
  appliedRuntimeEventIds: Set<string>;
  selectedAgentThreadId: string | null;
  selectedAgentView: AgentThreadView | null;
  selectedAgentUnlisten: Unlisten | null;
  selectedAgentAppliedEventIds: Set<string>;
  pendingApprovals: PendingApprovalItem[];
  approvalsStatus: ApprovalsStatus;
  approvalsError: string | null;
  approvalActionStatus: ApprovalActionStatus | null;
  approvalInboxOpen: boolean;
  selectedApprovalIds: Set<string>;
  compareThreadId: string | null;
  compareView: BranchCompareView | null;
  loadWorkbench: () => Promise<void>;
  addProject: () => Promise<void>;
  renameProject: (projectId: string, name: string) => Promise<void>;
  pinProject: (projectId: string, pinned: boolean) => Promise<void>;
  archiveProject: (projectId: string) => Promise<void>;
  removeProject: (projectId: string) => Promise<void>;
  archiveProjectConversations: (projectId: string) => Promise<void>;
  createProjectWorktree: (projectId: string) => Promise<void>;
  selectProject: (projectId: string, sessionId?: string) => Promise<void>;
  openSession: (sessionId: string) => Promise<void>;
  startSession: (projectId?: string) => Promise<string | null>;
  startPersonalSession: () => Promise<string | null>;
  sendPrompt: () => Promise<void>;
  interruptActiveTurn: () => Promise<void>;
  compactActiveThread: () => Promise<void>;
  forkThreadFromTurn: (threadId: string, turnId: string) => Promise<void>;
  openThreadGoalEditor: () => void;
  closeThreadGoalEditor: () => void;
  saveThreadGoal: (objective: string, tokenBudget?: number | null, mode?: ThreadGoalMode) => Promise<void>;
  setThreadGoalStatus: (status: Extract<ThreadGoalStatus, "active" | "paused" | "blocked" | "complete">) => Promise<void>;
  clearThreadGoal: () => Promise<void>;
  refreshAgentTree: () => Promise<void>;
  refreshApprovals: () => Promise<void>;
  setApprovalInboxOpen: (open: boolean) => void;
  toggleApprovalSelection: (approvalId: string) => void;
  clearApprovalSelection: () => void;
  approveInboxApproval: (item: PendingApprovalItem) => Promise<void>;
  rejectInboxApproval: (item: PendingApprovalItem) => Promise<void>;
  resolveOpenQuestion: (item: PendingApprovalItem, answer: string) => Promise<void>;
  approveSelectedApprovals: () => Promise<void>;
  rejectAndRollbackApproval: (item: PendingApprovalItem) => Promise<void>;
  submitApproval: (message: TranscriptMessage, decision: "approved" | "denied") => Promise<void>;
  submitUserInput: (message: TranscriptMessage, answers: string[][], dismissed: boolean) => Promise<void>;
  renameSession: (sessionId: string, title: string) => Promise<void>;
  archiveSession: (sessionId: string) => Promise<void>;
  unarchiveSession: (projectId: string, sessionId: string) => Promise<void>;
  openArchivedSession: (projectId: string, sessionId: string) => Promise<void>;
  pinSession: (sessionId: string, pinned: boolean) => Promise<void>;
  openBranchCompare: (sessionId: string, projectId?: string) => Promise<void>;
  closeCompareView: () => void;
  setComposerValue: (composerValue: string) => void;
  addComposerAttachments: (paths: string[]) => void;
  removeComposerAttachment: (id: string) => void;
  composerPlanMode: boolean;
  setComposerPlanMode: (enabled: boolean) => void;
  setSelectedModel: (model: string | ModelRef | null) => void;
  setSelectedThinkingMode: (thinkingMode: ThinkingMode | null) => void;
  applyProviderSettings: (settings: ProviderSettingsResponse) => void;
  applyRuntimePreset: (presetId: string) => void;
  saveRuntimeSettings: (settings: RuntimeSettingsSaveRequest) => Promise<void>;
  setSearch: (search: string) => Promise<void>;
  applyRuntimeEvent: (event: BackendRuntimeEvent) => void;
  applyRuntimeEvents: (events: BackendRuntimeEvent[]) => void;
  openAgentThread: (threadId: string) => Promise<void>;
  closeAgentThread: () => void;
  applySelectedAgentRuntimeEvents: (events: BackendRuntimeEvent[]) => void;
};

const emptySnapshot: WorkbenchSnapshot = {
  projects: [],
  sessions: [],
  activeProjectId: null,
  activeSessionId: null,
  activeTurnId: null,
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
  tokenUsageByThreadId: {},
  runtimeSettings: null,
  selectedModel: null,
  selectedThinkingMode: null
};

export const useWorkbenchStore = create<WorkbenchState>((set, get) => ({
  ...emptySnapshot,
  loading: true,
  error: null,
  agents: [],
  composerValue: "",
  composerAttachments: [],
  composerPlanMode: false,
  currentGoal: null,
  currentGoalMode: "standard",
  draftGoal: null,
  goalEditorOpen: false,
  search: "",
  activeProviderId: DEFAULT_PROVIDER_ID,
  providerSettings: null,
  selectionByThreadId: {},
  eventUnlisten: null,
  appliedRuntimeEventIds: new Set(),
  selectedAgentThreadId: null,
  selectedAgentView: null,
  selectedAgentUnlisten: null,
  selectedAgentAppliedEventIds: new Set(),
  pendingApprovals: [],
  approvalsStatus: "idle",
  approvalsError: null,
  approvalActionStatus: null,
  approvalInboxOpen: false,
  selectedApprovalIds: new Set(),
  compareThreadId: null,
  compareView: null,

  async loadWorkbench() {
    cancelPendingAgentTreeRefresh();
    cancelPendingApprovalsRefresh();
    resetSelectedAgentThread(get, set);
    set({ loading: true, error: null });
    try {
      const snapshot = await exagentClient.getWorkbenchSnapshot();
      const [runtimeSettings, providerSettings] = await Promise.all([
        exagentClient.getRuntimeSettings(),
        exagentClient.getProviderSettings()
      ]);
      const normalized = normalizeWorkbenchSelection({
        providerSettings,
        runtimeSettings,
        selectedModel: providerConfigModelRef(providerSettings),
        selectedThinkingMode: runtimeSettings.default_thinking_mode
      });
      set({
        ...snapshot,
        activeTurnId: snapshot.activeTurnId ?? null,
        runtimeSettings,
        providerSettings,
        selectedModel: normalized.selectedModel,
        selectedThinkingMode: normalized.selectedThinkingMode,
        activeProviderId: normalized.activeProviderId,
        draftGoal: null,
        currentGoalMode: "standard",
        appliedRuntimeEventIds: new Set(),
        compareThreadId: null,
        compareView: null,
        loading: false,
        error: null
      });
      if (snapshot.activeSessionId && exagentClient.isDesktopRuntime()) {
        await get().openSession(snapshot.activeSessionId);
      } else if (snapshot.activeSessionId) {
        // Browser preview: no live runtime, so seed a sample agent tree.
        set({ agents: agentForestFromTreeResponse(exagentClient.mockAgentTree(snapshot.activeSessionId)) });
      }
      await get().refreshApprovals();
    } catch (error) {
      set({
        ...emptySnapshot,
        activeProviderId: DEFAULT_PROVIDER_ID,
        providerSettings: null,
        draftGoal: null,
        currentGoalMode: "standard",
        appliedRuntimeEventIds: new Set(),
        pendingApprovals: [],
        approvalsStatus: "idle",
        approvalsError: null,
        approvalActionStatus: null,
        selectedApprovalIds: new Set(),
        compareThreadId: null,
        compareView: null,
        loading: false,
        error: errorMessage(error)
      });
    }
  },

  async addProject() {
    resetSelectedAgentThread(get, set);
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
        activeTurnId: null,
        transcript: [],
        currentGoal: null,
        currentGoalMode: "standard",
        draftGoal: null,
        events: [],
        cwd: project.path
      });
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async renameProject(projectId: string, name: string) {
    const nextName = name.trim();
    if (!nextName) {
      return;
    }
    try {
      const project = await exagentClient.renameProject(projectId, nextName);
      set({
        projects: get().projects.map((item) =>
          item.id === projectId
            ? { ...item, name: project.name, pinned: project.pinned, archived: project.archived_at !== null }
            : item
        ),
        cwd: get().activeProjectId === projectId ? project.path : get().cwd,
        error: null
      });
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async pinProject(projectId: string, pinned: boolean) {
    try {
      await exagentClient.pinProject(projectId, pinned);
      await refreshProjectSelection(get, set, get().activeProjectId);
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async archiveProject(projectId: string) {
    try {
      await exagentClient.archiveProject(projectId);
      await refreshProjectSelection(get, set, get().activeProjectId);
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async removeProject(projectId: string) {
    try {
      await exagentClient.removeProject(projectId);
      await refreshProjectSelection(get, set, get().activeProjectId);
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async archiveProjectConversations(projectId: string) {
    try {
      await exagentClient.archiveProjectConversations(projectId);
      if (projectId !== get().activeProjectId) {
        set({ error: null });
        return;
      }
      resetSelectedAgentThread(get, set);
      const threads = await exagentClient.listThreads(projectId, false, get().search || null);
      set({
        sessions: threads.map(exagentClient.threadRecordToSession),
        activeSessionId: null,
        activeTurnId: null,
        transcript: [],
        events: [],
        changedFiles: [],
        currentGoal: null,
        currentGoalMode: "standard",
        draftGoal: null,
        appliedRuntimeEventIds: new Set(),
        compareThreadId: null,
        compareView: null,
        error: null
      });
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async createProjectWorktree(projectId: string) {
    try {
      const project = await exagentClient.createProjectWorktree(projectId);
      await refreshProjectSelection(get, set, project.id);
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async selectProject(projectId: string, sessionId?: string) {
    const project = get().projects.find((item) => item.id === projectId);
    if (!project) {
      return;
    }
    cancelPendingApprovalsRefresh();
    resetSelectedAgentThread(get, set);
    set({ loading: true, error: null, compareThreadId: null, compareView: null });
    try {
      const threads = get().search
        ? await exagentClient.listThreads(projectId, false, get().search)
        : await exagentClient.reindexProject(projectId);
      const targetSessionId =
        sessionId && threads.some((thread) => thread.id === sessionId)
          ? sessionId
          : threads[0]?.id ?? null;
      set({
        projects: get().projects.map((item) => ({ ...item, active: item.id === projectId })),
        sessions: threads.map(exagentClient.threadRecordToSession),
        activeProjectId: projectId,
        activeSessionId: targetSessionId,
        activeTurnId: null,
        transcript: [],
        events: [],
        changedFiles: [],
        currentGoal: null,
        currentGoalMode: "standard",
        draftGoal: null,
        cwd: project.path,
        appliedRuntimeEventIds: new Set(),
        pendingApprovals: [],
        approvalsStatus: "idle",
        approvalsError: null,
        approvalActionStatus: null,
        selectedApprovalIds: new Set(),
        compareThreadId: null,
        compareView: null,
        loading: false
      });
      if (targetSessionId) {
        await get().openSession(targetSessionId);
      } else {
        await get().refreshApprovals();
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
    cancelPendingAgentTreeRefresh();
    cancelPendingApprovalsRefresh();
    const requestId = ++openSessionRequestSequence;
    resetSelectedAgentThread(get, set);
    get().eventUnlisten?.();
    const restoredSelection = selectionForThread(get(), sessionId);
    set({
      activeSessionId: sessionId,
      activeTurnId: null,
      loading: true,
      error: null,
      activeProviderId: restoredSelection.activeProviderId,
      selectedModel: restoredSelection.selectedModel,
      selectedThinkingMode: restoredSelection.selectedThinkingMode,
      eventUnlisten: null,
      events: [],
      currentGoal: null,
      currentGoalMode: "standard",
      draftGoal: null,
      agents: [],
      pendingApprovals: [],
      approvalsStatus: "idle",
      approvalsError: null,
      approvalActionStatus: null,
      selectedApprovalIds: new Set(),
      appliedRuntimeEventIds: new Set(),
      compareThreadId: null,
      compareView: null
    });

    const isCurrentOpen = () =>
      openSessionRequestSequence === requestId && get().activeSessionId === sessionId;
    const liveEventBatcher = createRuntimeEventBatcher((events) => {
      if (!isCurrentOpen()) {
        return;
      }
      get().applyRuntimeEvents(events.filter((event) => event.thread_id === sessionId));
    });
    let unlisten: Unlisten | null = null;
    let unlistenCalled = false;
    const cleanupUnlisten = () => {
      liveEventBatcher.cancel();
      cancelPendingAgentTreeRefresh({ projectId, threadId: sessionId, sessionGeneration: requestId });
      cancelPendingApprovalsRefresh({ projectId, sessionGeneration: requestId });
      if (!unlisten || unlistenCalled) {
        return;
      }
      unlistenCalled = true;
      unlisten();
    };

    try {
      const read = await exagentClient.resumeThread(projectId, sessionId);
      if (!isCurrentOpen()) {
        cleanupUnlisten();
        return;
      }
      const representedEventIds = threadViewEventIds(read.thread);
      const readStatus = sessionStatusFromThreadStatus(read.thread.status);
      const readSelection = selectionForThread(get(), sessionId, threadViewSelection(read.thread));
      const readGoal = visibleCurrentGoal(read.thread.goal ?? null);
      const readGoalMode = readGoal ? read.thread.goal_mode ?? "standard" : "standard";
      set({
        sessions: updateSessionStatus(get().sessions, sessionId, readStatus),
        activeTurnId: activeTurnIdFromThread(read.thread),
        transcript: threadViewToTranscript(read.thread),
        currentGoal: readGoal,
        currentGoalMode: readGoalMode,
        agents: buildAgentForest(sessionId, rootAgentStatus(readStatus), agentRecordsFromThreadView(read.thread)),
        activeProviderId: readSelection.activeProviderId,
        selectedModel: readSelection.selectedModel,
        selectedThinkingMode: readSelection.selectedThinkingMode,
        selectionByThreadId: withThreadSelection(get().selectionByThreadId, sessionId, readSelection),
        appliedRuntimeEventIds: representedEventIds,
        loading: false
      });

      const bufferedEvents: BackendRuntimeEvent[] = [];
      const bufferedEventIds = new Set<string>();
      let replayComplete = false;
      unlisten = await exagentClient.subscribeRuntimeEvents(projectId, sessionId, (event) => {
        if (!isCurrentOpen()) {
          return;
        }
        if (event.thread_id !== sessionId) {
          if (replayComplete && shouldRefreshAgentTreeAfterEvent(event)) {
            scheduleAgentTreeRefresh(get, set);
          }
          if (replayComplete && shouldRefreshApprovalsAfterEvent(event)) {
            scheduleApprovalsRefresh(get, set);
          }
          return;
        }
        if (!replayComplete) {
          if (!bufferedEventIds.has(event.event_id)) {
            bufferedEventIds.add(event.event_id);
            bufferedEvents.push(event);
          }
          return;
        }
        if (isCurrentOpen()) {
          liveEventBatcher.push(event);
        }
      });
      if (!isCurrentOpen()) {
        cleanupUnlisten();
        return;
      }
      set({ eventUnlisten: unlisten ? cleanupUnlisten : null });

      const replay = await exagentClient.replayEvents(projectId, sessionId, null);
      if (!isCurrentOpen()) {
        cleanupUnlisten();
        return;
      }

      const replayEventIds = new Set(replay.events.map((event) => event.event_id));
      const inspectorLiveEvents = bufferedEvents.filter((event) => !replayEventIds.has(event.event_id));
      const appliedRuntimeEventIds = new Set([
        ...representedEventIds,
        ...bufferedEvents.map((event) => event.event_id),
        ...replay.events.map((event) => event.event_id)
      ]);
      replayComplete = true;
      let agentRecords = agentRecordsFromThreadView(read.thread);
      let sessionStatus = sessionStatusFromThreadStatus(read.thread.status);
      for (const event of replay.events) {
        agentRecords = applyAgentEvent(agentRecords, event);
        if (event.thread_id === sessionId) {
          sessionStatus = statusFromEvent(event, sessionStatus);
        }
      }
      for (const event of inspectorLiveEvents) {
        agentRecords = applyAgentEvent(agentRecords, event);
        if (event.thread_id === sessionId) {
          sessionStatus = statusFromEvent(event, sessionStatus);
        }
      }
      const sessions = updateSessionStatus(get().sessions, sessionId, sessionStatus);
      let agents = buildAgentForest(sessionId, rootAgentStatus(sessionStatus), agentRecords);
      const activeTurnId = applyActiveTurnEvents(
        activeTurnIdFromThread(read.thread),
        [...replay.events, ...inspectorLiveEvents],
        sessionId
      );
      const currentGoal = applyGoalRuntimeEvents(readGoal, [
        ...replay.events,
        ...inspectorLiveEvents
      ]);
      const currentGoalMode = applyGoalModeRuntimeEvents(readGoalMode, [
        ...replay.events,
        ...inspectorLiveEvents
      ]);
      const tokenUsageByThreadId = applyTokenUsageEvents(get().tokenUsageByThreadId, [
        ...replay.events,
        ...inspectorLiveEvents
      ]);
      try {
        agents = applyAgentTokenUsageMap(
          agentForestFromTreeResponse(await exagentClient.agentTree(projectId, sessionId)),
          tokenUsageByThreadId
        );
        if (!isCurrentOpen()) {
          cleanupUnlisten();
          return;
        }
      } catch {
        // Keep the event-derived fallback when the app-server tree endpoint is unavailable.
      }
      if (!isCurrentOpen()) {
        cleanupUnlisten();
        return;
      }

      set({
        transcript: applyTranscriptEvents(threadViewToTranscript(read.thread), bufferedEvents, representedEventIds),
        activeTurnId,
        currentGoal: applyGoalRuntimeEvents(currentGoal, bufferedEvents),
        currentGoalMode: applyGoalModeRuntimeEvents(currentGoalMode, bufferedEvents),
        sessions,
        events: [
          ...runtimeEventsToInspector(inspectorLiveEvents).reverse(),
          ...runtimeEventsToInspector(replay.events)
        ].slice(0, 80),
        agents,
        appliedRuntimeEventIds,
        eventUnlisten: unlisten ? cleanupUnlisten : null,
        tokenUsageByThreadId,
        loading: false
      });
      await get().refreshApprovals();
    } catch (error) {
      cleanupUnlisten();
      if (isCurrentOpen()) {
        set({ error: errorMessage(error), eventUnlisten: null, loading: false });
      }
    }
  },

  async startSession(projectId?: string) {
    const targetProjectId = projectId ?? get().activeProjectId;
    if (!targetProjectId) {
      return null;
    }
    const targetIsActiveProject = targetProjectId === get().activeProjectId;
    const project = get().projects.find((item) => item.id === targetProjectId);
    if (!project && !targetIsActiveProject) {
      return null;
    }
    openSessionRequestSequence += 1;
    cancelPendingAgentTreeRefresh();
    cancelPendingApprovalsRefresh();
    resetSelectedAgentThread(get, set);
    get().eventUnlisten?.();

    if (!targetIsActiveProject) {
      set({
        loading: true,
        error: null,
        eventUnlisten: null,
        transcript: [],
        events: [],
        changedFiles: [],
        composerValue: "",
        composerAttachments: [],
        composerPlanMode: false,
        currentGoal: null,
        currentGoalMode: "standard",
        draftGoal: null,
        goalEditorOpen: false,
        appliedRuntimeEventIds: new Set(),
        compareThreadId: null,
        compareView: null
      });
      try {
        const threads = get().search
          ? await exagentClient.listThreads(targetProjectId, false, get().search)
          : await exagentClient.reindexProject(targetProjectId);
        set({
          projects: get().projects.map((item) => ({ ...item, active: item.id === targetProjectId })),
          sessions: threads.map(exagentClient.threadRecordToSession),
          activeProjectId: targetProjectId,
          activeSessionId: null,
          activeTurnId: null,
          transcript: [],
          currentGoal: null,
          currentGoalMode: "standard",
          draftGoal: null,
          goalEditorOpen: false,
          events: [],
          changedFiles: [],
          cwd: project?.path ?? get().cwd,
          eventUnlisten: null,
          appliedRuntimeEventIds: new Set(),
          pendingApprovals: [],
          approvalsStatus: "idle",
          approvalsError: null,
          approvalActionStatus: null,
          selectedApprovalIds: new Set(),
          compareThreadId: null,
          compareView: null,
          loading: false,
          error: null
        });
        await get().refreshApprovals();
      } catch (error) {
        set({ loading: false, error: errorMessage(error) });
      }
      return null;
    }

    set({
      activeSessionId: null,
      activeTurnId: null,
      transcript: [],
      currentGoal: null,
      currentGoalMode: "standard",
      draftGoal: null,
      goalEditorOpen: false,
      events: [],
      changedFiles: [],
      composerValue: "",
      composerAttachments: [],
      composerPlanMode: false,
      eventUnlisten: null,
      appliedRuntimeEventIds: new Set(),
      pendingApprovals: [],
      approvalsStatus: "idle",
      approvalsError: null,
      approvalActionStatus: null,
      selectedApprovalIds: new Set(),
      compareThreadId: null,
      compareView: null,
      error: null
    });
    return null;
  },

  async startPersonalSession() {
    set({ error: null });
    try {
      const project = await exagentClient.getOrCreatePersonalProject();
      const projects = await exagentClient.listProjects();
      set({
        projects: projects.map((item) => projectRecordToSummary(item, item.id === project.id)),
        activeProjectId: project.id,
        cwd: project.path,
        error: null
      });
      return await get().startSession(project.id);
    } catch (error) {
      set({ loading: false, error: errorMessage(error) });
      return null;
    }
  },

  async sendPrompt() {
    const prompt = get().composerValue.trim();
    const attachments = get().composerAttachments;
    if (!prompt && attachments.length === 0) {
      return;
    }
    const input = buildTurnInput(prompt, attachments);
    const inputForTurn = attachments.length > 0 ? input : [];
    const normalized = normalizeActiveWorkbenchSelection(get());
    if (inputContainsImage(input) && !selectedModelInputModalities(normalized, get()).includes("image")) {
      set({
        error: "The selected model does not support image input. Choose a vision-capable model before sending photos."
      });
      return;
    }
    const planMode = get().composerPlanMode;
    const projectId = get().activeProjectId;
    if (!projectId) {
      set({ error: "Choose a project folder first." });
      return;
    }
    let threadId = get().activeSessionId;
    const draftGoal = threadId ? null : get().draftGoal;
    if (!threadId) {
      try {
        const started = await exagentClient.startThread(projectId);
        const threads = await exagentClient.listThreads(projectId, false, get().search || null);
        threadId = started.thread.id;
        set({
          sessions: threads.map(exagentClient.threadRecordToSession),
          activeSessionId: threadId,
          events: [],
          changedFiles: [],
          selectionByThreadId: withThreadSelection(get().selectionByThreadId, threadId, normalized),
          appliedRuntimeEventIds: new Set()
        });
        await get().openSession(threadId);
      } catch (error) {
        set({ error: errorMessage(error) });
        return;
      }
    }
    if (!threadId) {
      return;
    }
    if (draftGoal) {
      const applied = await persistDraftGoal(projectId, threadId, draftGoal, set);
      if (!applied) {
        return;
      }
    }

    const optimisticMessage: TranscriptMessage = {
      id: `user-${Date.now()}`,
      role: "user",
      body: prompt,
      input: inputForTurn,
      timestamp: "now",
      threadId
    };
    set({
      composerValue: "",
      composerAttachments: [],
      composerPlanMode: false,
      transcript: [...get().transcript, optimisticMessage],
      sessions: get().sessions.map((session) =>
        session.id === threadId ? { ...session, status: "running" } : session
      )
    });

    try {
      const thinkingOverride = turnThinkingOverride(get(), normalized);
      set({
        activeProviderId: normalized.activeProviderId,
        selectedModel: normalized.selectedModel,
        selectedThinkingMode: normalized.selectedThinkingMode,
        selectionByThreadId: withThreadSelection(get().selectionByThreadId, threadId, normalized)
      });
      const started = await exagentClient.startTurn(
        projectId,
        threadId,
        prompt,
        {
          model: normalized.selectedModel,
          thinkingMode: thinkingOverride.thinkingMode,
          clearThinkingMode: thinkingOverride.clearThinkingMode,
          turnMode: planMode ? "plan" : "default",
          ...(inputForTurn.length > 0 ? { input: inputForTurn } : {})
        }
      );
      if (get().activeProjectId === projectId && get().activeSessionId === threadId) {
        set({
          activeTurnId: started.turn.id,
          transcript: get().transcript.map((message) =>
            message.id === optimisticMessage.id
              ? { ...message, turnId: started.turn.id, turnStatus: started.turn.status }
              : message
          )
        });
      }
    } catch (error) {
      const current = get();
      const shouldRestoreDraft = current.composerValue.length === 0 && current.composerAttachments.length === 0;
      set({
        error: errorMessage(error),
        ...(shouldRestoreDraft
          ? {
              composerValue: prompt,
              composerAttachments: attachments,
              composerPlanMode: planMode
            }
          : {}),
        transcript: current.transcript.filter((message) => message.id !== optimisticMessage.id),
        sessions: updateSessionStatus(current.sessions, threadId, "idle")
      });
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
      markSessionStatus(set, get, threadId, "idle");
      await get().refreshAgentTree();
    } catch (error) {
      if (isNoActiveTurnError(error)) {
        markSessionStatus(set, get, threadId, "idle");
        await get().refreshAgentTree();
        set({ error: null });
        return;
      }
      set({ error: errorMessage(error) });
    }
  },

  async compactActiveThread() {
    const projectId = get().activeProjectId;
    const threadId = get().activeSessionId;
    if (!projectId || !threadId) {
      return;
    }
    try {
      await exagentClient.compactThread(projectId, threadId);
      const replay = await exagentClient.replayEvents(projectId, threadId, null);
      if (get().activeProjectId !== projectId || get().activeSessionId !== threadId) {
        return;
      }
      get().applyRuntimeEvents(replay.events);
      set({ error: null });
    } catch (error) {
      if (get().activeProjectId === projectId && get().activeSessionId === threadId) {
        set({ error: errorMessage(error) });
      }
    }
  },

  async forkThreadFromTurn(threadId: string, turnId: string) {
    const projectId = get().activeProjectId;
    if (!projectId || !threadId || !turnId) {
      return;
    }
    try {
      const forked = await exagentClient.forkThread(projectId, {
        threadId,
        atTurnId: turnId
      });
      const threads = await exagentClient.listThreads(projectId, false, null);
      set({
        search: "",
        sessions: threads.map(exagentClient.threadRecordToSession),
        error: null
      });
      await get().openSession(forked.new_thread_id);
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  openThreadGoalEditor() {
    set({ goalEditorOpen: true });
  },

  closeThreadGoalEditor() {
    set({ goalEditorOpen: false });
  },

  async saveThreadGoal(objective: string, tokenBudget?: number | null, mode?: ThreadGoalMode) {
    const projectId = get().activeProjectId;
    const threadId = get().activeSessionId;
    const trimmedObjective = objective.trim();
    const selectedMode = mode ?? get().currentGoalMode;
    if (!projectId) {
      set({ error: "Choose a project folder before starting a goal." });
      return;
    }
    if (!trimmedObjective) {
      set({ error: "Goal objective cannot be empty." });
      return;
    }
    if (!threadId) {
      set({
        draftGoal: {
          objective: trimmedObjective,
          token_budget: tokenBudget ?? null,
          mode: selectedMode
        },
        goalEditorOpen: false,
        error: null
      });
      return;
    }
    try {
      const response = await exagentClient.setThreadGoal(projectId, threadId, {
        objective: trimmedObjective,
        status: "active",
        tokenBudget: tokenBudget ?? null,
        clearTokenBudget: tokenBudget === null,
        mode: selectedMode
      });
      set({
        currentGoal: response.goal,
        currentGoalMode: response.mode,
        goalEditorOpen: false,
        error: null
      });
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async setThreadGoalStatus(status) {
    const projectId = get().activeProjectId;
    const threadId = get().activeSessionId;
    if (!projectId || !threadId) {
      return;
    }
    try {
      const response = await exagentClient.setThreadGoal(projectId, threadId, { status });
      set({ currentGoal: response.goal, currentGoalMode: response.mode, error: null });
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async clearThreadGoal() {
    const projectId = get().activeProjectId;
    const threadId = get().activeSessionId;
    if (!threadId) {
      set({ draftGoal: null, currentGoalMode: "standard", goalEditorOpen: false, error: null });
      return;
    }
    if (!projectId) {
      return;
    }
    try {
      await exagentClient.clearThreadGoal(projectId, threadId);
      set({ currentGoal: null, currentGoalMode: "standard", goalEditorOpen: false, error: null });
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async refreshAgentTree() {
    const context = currentAgentTreeRefreshContext(get);
    if (!context) {
      return;
    }
    await refreshAgentTreeForContext(get, set, context);
  },

  async refreshApprovals() {
    const context = currentApprovalsRefreshContext(get);
    if (!context) {
      set({ pendingApprovals: [], approvalsStatus: "idle", approvalsError: null, selectedApprovalIds: new Set() });
      return;
    }
    await refreshApprovalsForContext(get, set, context);
  },

  setApprovalInboxOpen(open: boolean) {
    set({ approvalInboxOpen: open });
    if (open && get().approvalsStatus !== "ready" && get().approvalsStatus !== "submitting") {
      void get().refreshApprovals();
    }
  },

  toggleApprovalSelection(approvalId: string) {
    const selectedApprovalIds = new Set(get().selectedApprovalIds);
    if (selectedApprovalIds.has(approvalId)) {
      selectedApprovalIds.delete(approvalId);
    } else {
      selectedApprovalIds.add(approvalId);
    }
    set({ selectedApprovalIds });
  },

  clearApprovalSelection() {
    set({ selectedApprovalIds: new Set() });
  },

  async approveInboxApproval(item: PendingApprovalItem) {
    if (item.kind === "open_question") {
      return;
    }
    await submitInboxApprovalDecision(get, set, item, "approved", "desktop approved");
  },

  async rejectInboxApproval(item: PendingApprovalItem) {
    if (item.kind === "open_question") {
      return;
    }
    await submitInboxApprovalDecision(get, set, item, "denied", "desktop denied");
  },

  async resolveOpenQuestion(item: PendingApprovalItem, answer: string) {
    await submitInboxOpenQuestionResolution(get, set, item, answer);
  },

  async approveSelectedApprovals() {
    const context = currentApprovalsRefreshContext(get);
    if (!context) {
      return;
    }
    const selectedIds = new Set(get().selectedApprovalIds);
    const selected = get().pendingApprovals.filter(
      (item) => selectedIds.has(item.approval_id) && item.kind === "command"
    );
    if (selected.length === 0) {
      return;
    }

    set({ approvalsStatus: "submitting", approvalsError: null, approvalActionStatus: null });
    let completed = 0;
    for (const item of selected) {
      try {
        await exagentClient.submitApprovalDecision(
          context.projectId,
          item.thread_id,
          undefined,
          item.approval_id,
          "approved",
          "desktop approved"
        );
        if (!isActiveApprovalsRefreshContext(get, context)) {
          return;
        }
        completed += 1;
        set({
          pendingApprovals: get().pendingApprovals.filter((pending) => pending.approval_id !== item.approval_id),
          selectedApprovalIds: removeApprovalId(get().selectedApprovalIds, item.approval_id)
        });
      } catch (error) {
        if (!isActiveApprovalsRefreshContext(get, context)) {
          return;
        }
        set({
          approvalsStatus: "error",
          approvalsError: null,
          approvalActionStatus: {
            type: "batch_partial_failed",
            completed,
            total: selected.length,
            approval_id: item.approval_id,
            error: errorMessage(error)
          }
        });
        await refreshApprovalsForContext(get, set, context);
        return;
      }
    }

    if (!isActiveApprovalsRefreshContext(get, context)) {
      return;
    }
    set({
      approvalsStatus: "ready",
      approvalActionStatus: { type: "batch_approved", count: completed },
      approvalsError: null,
      selectedApprovalIds: new Set()
    });
    await refreshApprovalsForContext(get, set, context);
  },

  async rejectAndRollbackApproval(item: PendingApprovalItem) {
    const context = currentApprovalsRefreshContext(get);
    if (!context || !item.checkpoint_id) {
      set({
        approvalsStatus: "error",
        approvalsError: null,
        approvalActionStatus: { type: "rollback_unavailable", approval_id: item.approval_id }
      });
      return;
    }

    set({ approvalsStatus: "submitting", approvalsError: null, approvalActionStatus: null });
    let denied = false;
    try {
      await exagentClient.submitApprovalDecision(
        context.projectId,
        item.thread_id,
        undefined,
        item.approval_id,
        "denied",
        "desktop denied"
      );
      denied = true;
      await exagentClient.restoreCheckpoint(context.projectId, item.checkpoint_id);
      if (!isActiveApprovalsRefreshContext(get, context)) {
        return;
      }
      set({
        pendingApprovals: get().pendingApprovals.filter((pending) => pending.approval_id !== item.approval_id),
        selectedApprovalIds: removeApprovalId(get().selectedApprovalIds, item.approval_id),
        approvalsStatus: "ready",
        approvalsError: null,
        approvalActionStatus: {
          type: "rollback_restored",
          approval_id: item.approval_id,
          checkpoint_id: item.checkpoint_id
        }
      });
      await refreshApprovalsForContext(get, set, context);
    } catch (error) {
      if (!isActiveApprovalsRefreshContext(get, context)) {
        return;
      }
      if (denied) {
        set({
          pendingApprovals: get().pendingApprovals.filter((pending) => pending.approval_id !== item.approval_id),
          selectedApprovalIds: removeApprovalId(get().selectedApprovalIds, item.approval_id),
          approvalsStatus: "error",
          approvalsError: null,
          approvalActionStatus: {
            type: "rollback_failed_after_reject",
            approval_id: item.approval_id,
            error: errorMessage(error)
          }
        });
        await refreshApprovalsForContext(get, set, context);
      } else {
        set({ approvalsStatus: "error", approvalsError: errorMessage(error), approvalActionStatus: null });
      }
    }
  },

  async submitApproval(message: TranscriptMessage, decision: "approved" | "denied") {
    const context = currentApprovalsRefreshContext(get);
    const threadId = message.threadId ?? get().activeSessionId;
    if (!context || !threadId || !message.approvalId) {
      return;
    }
    try {
      await exagentClient.submitApprovalDecision(
        context.projectId,
        threadId,
        message.turnId,
        message.approvalId,
        decision,
        decision === "approved" ? "desktop approved" : "desktop denied"
      );
      if (isActiveApprovalsRefreshContext(get, context)) {
        void refreshApprovalsForContext(get, set, context);
      }
    } catch (error) {
      if (isActiveApprovalsRefreshContext(get, context)) {
        set({ error: errorMessage(error) });
      }
    }
  },

  async submitUserInput(message: TranscriptMessage, answers: string[][], dismissed: boolean) {
    const context = currentApprovalsRefreshContext(get);
    const threadId = message.threadId ?? get().activeSessionId;
    if (!context || !threadId || !message.requestId) {
      return;
    }
    try {
      await exagentClient.submitUserInput(
        context.projectId,
        threadId,
        message.turnId,
        message.requestId,
        answers,
        dismissed
      );
      if (isActiveApprovalsRefreshContext(get, context)) {
        void refreshApprovalsForContext(get, set, context);
      }
    } catch (error) {
      if (isActiveApprovalsRefreshContext(get, context)) {
        set({ error: errorMessage(error) });
      }
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
      const current = get();
      const active = current.activeSessionId === sessionId;
      const compareReferencesArchivedThread =
        current.compareThreadId === sessionId || compareViewReferencesThread(current.compareView, sessionId);
      if (compareReferencesArchivedThread) {
        branchCompareRequestSequence += 1;
      }
      if (active) {
        resetSelectedAgentThread(get, set);
      }
      set({
        sessions: current.sessions.filter((session) => session.id !== sessionId),
        activeSessionId: active ? null : current.activeSessionId,
        activeTurnId: active ? null : current.activeTurnId,
        transcript: active ? [] : current.transcript,
        events: active ? [] : current.events,
        changedFiles: active ? [] : current.changedFiles,
        appliedRuntimeEventIds: active ? new Set() : current.appliedRuntimeEventIds,
        compareThreadId: compareReferencesArchivedThread ? null : current.compareThreadId,
        compareView: compareReferencesArchivedThread ? null : current.compareView
      });
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async unarchiveSession(projectId: string, sessionId: string) {
    try {
      await exagentClient.unarchiveThread(sessionId);
      if (projectId !== get().activeProjectId) {
        set({ error: null });
        return;
      }
      const threads = await exagentClient.listThreads(projectId, false, get().search || null);
      set({
        sessions: threads.map(exagentClient.threadRecordToSession),
        error: null
      });
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  async openArchivedSession(projectId: string, sessionId: string) {
    try {
      await exagentClient.unarchiveThread(sessionId);
      await get().selectProject(projectId, sessionId);
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

  async openBranchCompare(sessionId: string, projectId?: string) {
    if (projectId && projectId !== get().activeProjectId) {
      await get().selectProject(projectId, sessionId);
      if (get().activeProjectId !== projectId) {
        return;
      }
    }

    const activeProjectId = get().activeProjectId;
    const childSession = get().sessions.find((session) => session.id === sessionId);
    const parentThreadId = childSession?.forkParentThreadId;
    const forkPointTurnId = childSession?.forkPointTurnId;
    if (!activeProjectId || !childSession || !parentThreadId || !forkPointTurnId) {
      return;
    }

    const parentSession = get().sessions.find((session) => session.id === parentThreadId);
    const requestId = ++branchCompareRequestSequence;
    const loadingView: BranchCompareView = {
      parentThreadId,
      childThreadId: childSession.id,
      parentTitle: parentSession?.title ?? parentThreadId,
      childTitle: childSession.title,
      parentTranscript: [],
      childTranscript: [],
      sharedTurnCount: 0,
      forkPointTurnId,
      loading: true,
      error: null
    };
    set({
      compareThreadId: childSession.id,
      compareView: loadingView,
      error: null
    });

    const isCurrentCompare = () =>
      branchCompareRequestSequence === requestId && get().compareThreadId === childSession.id;

    try {
      const [parentRead, childRead] = await Promise.all([
        exagentClient.readThread(activeProjectId, parentThreadId),
        exagentClient.readThread(activeProjectId, childSession.id)
      ]);
      if (!isCurrentCompare()) {
        return;
      }
      set({
        compareView: {
          ...loadingView,
          parentTranscript: postForkTranscript(parentRead.thread, forkPointTurnId),
          childTranscript: postForkTranscript(childRead.thread, forkPointTurnId),
          sharedTurnCount: sharedTurnCountForFork(parentRead.thread, childRead.thread, forkPointTurnId),
          loading: false,
          error: null
        }
      });
    } catch (error) {
      if (!isCurrentCompare()) {
        return;
      }
      set({
        compareView: {
          ...(get().compareView ?? loadingView),
          loading: false,
          error: errorMessage(error)
        }
      });
    }
  },

  closeCompareView() {
    branchCompareRequestSequence += 1;
    set({ compareThreadId: null, compareView: null });
  },

  setComposerValue(composerValue: string) {
    set({ composerValue });
  },

  addComposerAttachments(paths: string[]) {
    if (paths.length === 0) {
      return;
    }
    const seenPaths = new Set(get().composerAttachments.map((attachment) => attachment.path));
    const attachments: ComposerAttachment[] = [];
    for (const path of paths) {
      const normalizedPath = path.trim();
      if (!normalizedPath || seenPaths.has(normalizedPath)) {
        continue;
      }
      seenPaths.add(normalizedPath);
      attachments.push(composerAttachmentFromPath(normalizedPath));
    }
    if (attachments.length === 0) {
      return;
    }
    set({ composerAttachments: [...get().composerAttachments, ...attachments], error: null });
  },

  removeComposerAttachment(id: string) {
    set({
      composerAttachments: get().composerAttachments.filter((attachment) => attachment.id !== id)
    });
  },

  setComposerPlanMode(composerPlanMode: boolean) {
    set({ composerPlanMode });
  },

  setSelectedModel(model) {
    const normalized = normalizeWorkbenchSelection({
      ...get(),
      selectedModel: normalizeModelRef(model, get().activeProviderId)
    });
    set({
      activeProviderId: normalized.activeProviderId,
      selectedModel: normalized.selectedModel,
      selectedThinkingMode: normalized.selectedThinkingMode,
      selectionByThreadId: withThreadSelection(
        get().selectionByThreadId,
        get().activeSessionId,
        normalized
      )
    });
  },

  setSelectedThinkingMode(selectedThinkingMode) {
    const normalized = normalizeWorkbenchSelection({
      ...get(),
      selectedThinkingMode
    });
    set({
      selectedThinkingMode: normalized.selectedThinkingMode,
      selectionByThreadId: withThreadSelection(
        get().selectionByThreadId,
        get().activeSessionId,
        normalized
      )
    });
  },

  applyProviderSettings(settings) {
    const normalized = normalizeWorkbenchSelection({
      ...get(),
      providerSettings: settings,
      selectedModel: providerConfigModelRef(settings)
    });
    set({
      activeProviderId: normalized.activeProviderId,
      providerSettings: settings,
      selectedModel: normalized.selectedModel,
      selectedThinkingMode: normalized.selectedThinkingMode,
      selectionByThreadId: withThreadSelection(
        get().selectionByThreadId,
        get().activeSessionId,
        normalized
      )
    });
  },

  applyRuntimePreset(presetId) {
    const preset = get().runtimeSettings?.presets.find((item) => item.id === presetId);
    if (!preset) {
      return;
    }
    const normalized = normalizeWorkbenchSelection({
      ...get(),
      selectedModel: modelRefFromString(preset.model, get().activeProviderId),
      selectedThinkingMode: preset.thinking_mode
    });
    set({
      activeProviderId: normalized.activeProviderId,
      selectedModel: normalized.selectedModel,
      selectedThinkingMode: normalized.selectedThinkingMode,
      selectionByThreadId: withThreadSelection(
        get().selectionByThreadId,
        get().activeSessionId,
        normalized
      )
    });
  },

  async saveRuntimeSettings(settings) {
    try {
      const runtimeSettings = await exagentClient.saveRuntimeSettings(settings);
      const normalized = normalizeWorkbenchSelection({
        ...get(),
        runtimeSettings,
        selectedThinkingMode: runtimeSettings.default_thinking_mode
      });
      set({
        runtimeSettings,
        activeProviderId: normalized.activeProviderId,
        selectedModel: normalized.selectedModel,
        selectedThinkingMode: normalized.selectedThinkingMode,
        selectionByThreadId: withThreadSelection(
          get().selectionByThreadId,
          get().activeSessionId,
          normalized
        ),
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
    get().applyRuntimeEvents([event]);
  },

  applyRuntimeEvents(events: BackendRuntimeEvent[]) {
    if (events.length === 0) {
      return;
    }
    const current = get();
    const activeThreadId = current.activeSessionId;
    const appliedRuntimeEventIds = new Set(current.appliedRuntimeEventIds);
    let transcript = current.transcript;
    let currentGoal = current.currentGoal;
    let currentGoalMode = current.currentGoalMode;
    let inspectorEvents = current.events;
    let sessions = current.sessions;
    let agentRecords = flattenAgentForest(current.agents);
    let tokenUsageByThreadId = current.tokenUsageByThreadId;
    let activeTurnId = current.activeTurnId ?? null;
    let agentRecordsChanged = false;
    let changed = false;

    for (const event of events) {
      if (appliedRuntimeEventIds.has(event.event_id)) {
        continue;
      }
      appliedRuntimeEventIds.add(event.event_id);
      changed = true;
      tokenUsageByThreadId = applyTokenUsageEvents(tokenUsageByThreadId, [event]);

      sessions = sessions.map((session) => {
        if (session.id !== event.thread_id) {
          return session;
        }
        const nextStatus = statusFromEvent(event, session.status);
        return nextStatus === session.status ? session : { ...session, status: nextStatus };
      });

      if (event.thread_id === activeThreadId) {
        activeTurnId = applyActiveTurnEvent(activeTurnId, event);
        transcript = applyTranscriptEvent(transcript, event);
        currentGoal = applyGoalRuntimeEvent(currentGoal, event);
        currentGoalMode = applyGoalModeRuntimeEvent(currentGoalMode, event);
        const nextAgentRecords = applyAgentEvent(agentRecords, event);
        if (nextAgentRecords !== agentRecords) {
          agentRecords = nextAgentRecords;
          agentRecordsChanged = true;
        }
      }
      if (shouldShowInspectorEvent(event)) {
        inspectorEvents = [
          runtimeEventToInspector(event),
          ...inspectorEvents.filter((item) => item.id !== event.event_id)
        ].slice(0, 80);
      }
    }

    if (!changed) {
      return;
    }

    let agents = current.agents;
    if (activeThreadId) {
      const rootStatus = rootAgentStatus(
        sessions.find((session) => session.id === activeThreadId)?.status
      );
      const rootStatusChanged = current.agents[0]?.status !== rootStatus;
      if (agentRecordsChanged || rootStatusChanged || current.agents.length === 0) {
        agents = buildAgentForest(activeThreadId, rootStatus, agentRecords);
      }
    }
    agents = applyAgentTokenUsageMap(agents, tokenUsageByThreadId);

    set({
      transcript,
      currentGoal,
      currentGoalMode,
      events: inspectorEvents,
      agents,
      appliedRuntimeEventIds,
      tokenUsageByThreadId,
      activeTurnId,
      sessions
    });
    if (events.some(shouldRefreshAgentTreeAfterEvent)) {
      scheduleAgentTreeRefresh(get, set);
    }
    if (events.some(shouldRefreshApprovalsAfterEvent)) {
      scheduleApprovalsRefresh(get, set);
    }
  },

  async openAgentThread(threadId: string) {
    const projectId = get().activeProjectId;
    const activeThreadId = get().activeSessionId;
    if (!projectId || !activeThreadId || threadId === activeThreadId) {
      return;
    }

    const requestId = ++openAgentThreadRequestSequence;
    get().selectedAgentUnlisten?.();
    const selectedNode = findAgentNode(get().agents, threadId);
    set({
      selectedAgentThreadId: threadId,
      selectedAgentView: {
        threadId,
        transcript: [],
        events: [],
        loading: true,
        error: null
      },
      selectedAgentUnlisten: null,
      selectedAgentAppliedEventIds: new Set()
    });

    const isCurrentOpen = () =>
      openAgentThreadRequestSequence === requestId && get().selectedAgentThreadId === threadId;
    const liveEventBatcher = createRuntimeEventBatcher((events) => {
      if (!isCurrentOpen()) {
        return;
      }
      get().applySelectedAgentRuntimeEvents(events.filter((event) => event.thread_id === threadId));
    });
    let unlisten: Unlisten | null = null;
    let unlistenCalled = false;
    const cleanupUnlisten = () => {
      liveEventBatcher.cancel();
      if (!unlisten || unlistenCalled) {
        return;
      }
      unlistenCalled = true;
      unlisten();
    };

    try {
      const read = await exagentClient.readThread(projectId, threadId);
      if (!isCurrentOpen()) {
        cleanupUnlisten();
        return;
      }

      const representedEventIds = threadViewEventIds(read.thread);
      const baseTranscript = threadViewToTranscript(read.thread);
      const bufferedEvents: BackendRuntimeEvent[] = [];
      const bufferedEventIds = new Set<string>();
      let replayComplete = false;

      if (selectedNode?.status !== "done" && selectedNode?.status !== "failed") {
        unlisten = await exagentClient.subscribeRuntimeEvents(projectId, threadId, (event) => {
          if (!isCurrentOpen() || event.thread_id !== threadId) {
            return;
          }
          if (!replayComplete) {
            if (!bufferedEventIds.has(event.event_id)) {
              bufferedEventIds.add(event.event_id);
              bufferedEvents.push(event);
            }
            return;
          }
          liveEventBatcher.push(event);
        });
        if (!isCurrentOpen()) {
          cleanupUnlisten();
          return;
        }
        set({ selectedAgentUnlisten: unlisten ? cleanupUnlisten : null });
      }

      const replay = await exagentClient.replayEvents(projectId, threadId, null);
      if (!isCurrentOpen()) {
        cleanupUnlisten();
        return;
      }

      const replayEventIds = new Set(replay.events.map((event) => event.event_id));
      const bufferedLiveEvents = bufferedEvents.filter((event) => !replayEventIds.has(event.event_id));
      const appliedEventIds = new Set([
        ...representedEventIds,
        ...replay.events.map((event) => event.event_id),
        ...bufferedLiveEvents.map((event) => event.event_id)
      ]);
      replayComplete = true;
      const tokenUsageByThreadId = applyTokenUsageEvents(get().tokenUsageByThreadId, [
        ...replay.events,
        ...bufferedLiveEvents
      ]);

      set({
        selectedAgentView: {
          threadId,
          transcript: applyTranscriptEvents(
            baseTranscript,
            [...replay.events, ...bufferedLiveEvents],
            representedEventIds
          ),
          events: [
            ...runtimeEventsToInspector(bufferedLiveEvents).reverse(),
            ...runtimeEventsToInspector(replay.events)
          ].slice(0, 50),
          loading: false,
          error: null
        },
        selectedAgentAppliedEventIds: appliedEventIds,
        tokenUsageByThreadId,
        agents: applyAgentTokenUsageMap(get().agents, tokenUsageByThreadId),
        selectedAgentUnlisten: unlisten ? cleanupUnlisten : null
      });
      if (threadViewHasFailedTurn(read.thread) || replay.events.some((event) => event.kind.type === "runtime_error")) {
        void get().refreshAgentTree();
      }
    } catch (error) {
      cleanupUnlisten();
      if (isCurrentOpen()) {
        set({
          selectedAgentView: {
            threadId,
            transcript: get().selectedAgentView?.transcript ?? [],
            events: get().selectedAgentView?.events ?? [],
            loading: false,
            error: errorMessage(error)
          },
          selectedAgentUnlisten: null
        });
      }
    }
  },

  closeAgentThread() {
    resetSelectedAgentThread(get, set);
  },

  applySelectedAgentRuntimeEvents(events: BackendRuntimeEvent[]) {
    if (events.length === 0) {
      return;
    }

    const current = get();
    const threadId = current.selectedAgentThreadId;
    const currentView = current.selectedAgentView;
    if (!threadId || !currentView) {
      return;
    }

    const appliedEventIds = new Set(current.selectedAgentAppliedEventIds);
    let transcript = currentView.transcript;
    let inspectorEvents = currentView.events;
    let tokenUsageByThreadId = current.tokenUsageByThreadId;
    let changed = false;

    for (const event of events) {
      if (event.thread_id !== threadId || appliedEventIds.has(event.event_id)) {
        continue;
      }
      appliedEventIds.add(event.event_id);
      changed = true;
      tokenUsageByThreadId = applyTokenUsageEvents(tokenUsageByThreadId, [event]);
      transcript = applyTranscriptEvent(transcript, event);
      if (shouldShowInspectorEvent(event)) {
        inspectorEvents = [
          runtimeEventToInspector(event),
          ...inspectorEvents.filter((item) => item.id !== event.event_id)
        ].slice(0, 50);
      }
    }

    if (!changed) {
      return;
    }

    set({
      selectedAgentView: {
        ...currentView,
        transcript,
        events: inspectorEvents,
        loading: false,
        error: null
      },
      agents: applyAgentTokenUsageMap(current.agents, tokenUsageByThreadId),
      tokenUsageByThreadId,
      selectedAgentAppliedEventIds: appliedEventIds
    });
    if (events.some(shouldRefreshAgentTreeAfterEvent)) {
      scheduleAgentTreeRefresh(get, set);
    }
    if (events.some(shouldRefreshApprovalsAfterEvent)) {
      scheduleApprovalsRefresh(get, set);
    }
  }
}));

export function getWorkbenchState() {
  return useWorkbenchStore.getState();
}

export const loadWorkbench = () => useWorkbenchStore.getState().loadWorkbench();
export const setComposerValue = (composerValue: string) =>
  useWorkbenchStore.getState().setComposerValue(composerValue);
export const addComposerAttachments = (paths: string[]) =>
  useWorkbenchStore.getState().addComposerAttachments(paths);
export const removeComposerAttachment = (id: string) =>
  useWorkbenchStore.getState().removeComposerAttachment(id);
export const setComposerPlanMode = (enabled: boolean) =>
  useWorkbenchStore.getState().setComposerPlanMode(enabled);
export const setSelectedModel = (model: string | ModelRef | null) =>
  useWorkbenchStore.getState().setSelectedModel(model);
export const setSelectedThinkingMode = (thinkingMode: ThinkingMode | null) =>
  useWorkbenchStore.getState().setSelectedThinkingMode(thinkingMode);
export const applyProviderSettings = (settings: ProviderSettingsResponse) =>
  useWorkbenchStore.getState().applyProviderSettings(settings);
export const applyRuntimePreset = (presetId: string) =>
  useWorkbenchStore.getState().applyRuntimePreset(presetId);
export const sendPrompt = () => useWorkbenchStore.getState().sendPrompt();
export const interruptActiveTurn = () => useWorkbenchStore.getState().interruptActiveTurn();
export const compactActiveThread = () => useWorkbenchStore.getState().compactActiveThread();
export const openThreadGoalEditor = () => useWorkbenchStore.getState().openThreadGoalEditor();
export const closeThreadGoalEditor = () => useWorkbenchStore.getState().closeThreadGoalEditor();
export const saveThreadGoal = (objective: string, tokenBudget?: number | null, mode?: ThreadGoalMode) =>
  useWorkbenchStore.getState().saveThreadGoal(objective, tokenBudget, mode);
export const setThreadGoalStatus = (
  status: Extract<ThreadGoalStatus, "active" | "paused" | "blocked" | "complete">
) => useWorkbenchStore.getState().setThreadGoalStatus(status);
export const clearThreadGoal = () => useWorkbenchStore.getState().clearThreadGoal();
export const submitApproval = (message: TranscriptMessage, decision: "approved" | "denied") =>
  useWorkbenchStore.getState().submitApproval(message, decision);
export const submitUserInput = (message: TranscriptMessage, answers: string[][], dismissed: boolean) =>
  useWorkbenchStore.getState().submitUserInput(message, answers, dismissed);

function composerAttachmentFromPath(path: string): ComposerAttachment {
  const normalizedPath = path.trim();
  const name = normalizedPath.split(/[\\/]/).filter(Boolean).at(-1) ?? normalizedPath;
  return {
    id: `${normalizedPath}:${Date.now()}:${Math.random().toString(36).slice(2)}`,
    type: "local_image",
    path: normalizedPath,
    name,
    detail: "high"
  };
}

function buildTurnInput(prompt: string, attachments: ComposerAttachment[]): TurnInput[] {
  return [
    ...(prompt ? [{ type: "text" as const, text: prompt }] : []),
    ...attachments.map((attachment) => ({
      type: "local_image" as const,
      path: attachment.path,
      detail: attachment.detail
    }))
  ];
}

function inputContainsImage(input: TurnInput[]) {
  return input.some((part) => part.type === "local_image" || part.type === "image_url");
}

function createRuntimeEventBatcher(apply: (events: BackendRuntimeEvent[]) => void) {
  let queued: BackendRuntimeEvent[] = [];
  let frameId: number | null = null;
  let timeoutId: ReturnType<typeof setTimeout> | null = null;

  const flush = () => {
    frameId = null;
    timeoutId = null;
    const events = queued;
    queued = [];
    apply(events);
  };

  const schedule = () => {
    if (frameId !== null || timeoutId !== null) {
      return;
    }
    if (typeof globalThis.requestAnimationFrame === "function") {
      frameId = globalThis.requestAnimationFrame(flush);
      return;
    }
    timeoutId = setTimeout(flush, 16);
  };

  return {
    push(event: BackendRuntimeEvent) {
      queued.push(event);
      schedule();
    },
    cancel() {
      queued = [];
      if (frameId !== null && typeof globalThis.cancelAnimationFrame === "function") {
        globalThis.cancelAnimationFrame(frameId);
      }
      if (timeoutId !== null) {
        clearTimeout(timeoutId);
      }
      frameId = null;
      timeoutId = null;
    }
  };
}

function scheduleAgentTreeRefresh(
  get: () => WorkbenchState,
  set: (partial: Partial<WorkbenchState>) => void
) {
  const context = currentAgentTreeRefreshContext(get);
  if (!context) {
    return;
  }

  pendingAgentTreeRefreshContext = context;
  if (agentTreeRefreshTimeoutId !== null) {
    clearTimeout(agentTreeRefreshTimeoutId);
  }
  agentTreeRefreshTimeoutId = setTimeout(() => {
    const context = pendingAgentTreeRefreshContext;
    agentTreeRefreshTimeoutId = null;
    pendingAgentTreeRefreshContext = null;
    if (!context) {
      return;
    }
    void refreshAgentTreeForContext(get, set, context);
  }, AGENT_TREE_REFRESH_DEBOUNCE_MS);
}

function cancelPendingAgentTreeRefresh(context?: AgentTreeRefreshContext) {
  if (
    context &&
    pendingAgentTreeRefreshContext &&
    !sameAgentTreeRefreshContext(pendingAgentTreeRefreshContext, context)
  ) {
    return;
  }
  if (agentTreeRefreshTimeoutId !== null) {
    clearTimeout(agentTreeRefreshTimeoutId);
  }
  agentTreeRefreshTimeoutId = null;
  pendingAgentTreeRefreshContext = null;
}

function scheduleApprovalsRefresh(
  get: () => WorkbenchState,
  set: (partial: Partial<WorkbenchState>) => void
) {
  const context = currentApprovalsRefreshContext(get);
  if (!context) {
    return;
  }

  pendingApprovalsRefreshContext = context;
  if (approvalsRefreshTimeoutId !== null) {
    clearTimeout(approvalsRefreshTimeoutId);
  }
  approvalsRefreshTimeoutId = setTimeout(() => {
    const context = pendingApprovalsRefreshContext;
    approvalsRefreshTimeoutId = null;
    pendingApprovalsRefreshContext = null;
    if (!context) {
      return;
    }
    void refreshApprovalsForContext(get, set, context);
  }, APPROVALS_REFRESH_DEBOUNCE_MS);
}

function cancelPendingApprovalsRefresh(context?: ApprovalsRefreshContext) {
  if (
    context &&
    pendingApprovalsRefreshContext &&
    !sameApprovalsRefreshContext(pendingApprovalsRefreshContext, context)
  ) {
    return;
  }
  if (approvalsRefreshTimeoutId !== null) {
    clearTimeout(approvalsRefreshTimeoutId);
  }
  approvalsRefreshTimeoutId = null;
  pendingApprovalsRefreshContext = null;
}

function currentAgentTreeRefreshContext(get: () => WorkbenchState): AgentTreeRefreshContext | null {
  const projectId = get().activeProjectId;
  const threadId = get().activeSessionId;
  if (!projectId || !threadId) {
    return null;
  }
  return { projectId, threadId, sessionGeneration: openSessionRequestSequence };
}

function currentApprovalsRefreshContext(get: () => WorkbenchState): ApprovalsRefreshContext | null {
  const projectId = get().activeProjectId;
  if (!projectId) {
    return null;
  }
  return { projectId, sessionGeneration: openSessionRequestSequence };
}

function sameAgentTreeRefreshContext(a: AgentTreeRefreshContext, b: AgentTreeRefreshContext) {
  return a.projectId === b.projectId && a.threadId === b.threadId && a.sessionGeneration === b.sessionGeneration;
}

function sameApprovalsRefreshContext(a: ApprovalsRefreshContext, b: ApprovalsRefreshContext) {
  return a.projectId === b.projectId && a.sessionGeneration === b.sessionGeneration;
}

function isActiveAgentTreeRefreshContext(get: () => WorkbenchState, context: AgentTreeRefreshContext) {
  return (
    get().activeProjectId === context.projectId &&
    get().activeSessionId === context.threadId &&
    openSessionRequestSequence === context.sessionGeneration
  );
}

function isActiveApprovalsRefreshContext(get: () => WorkbenchState, context: ApprovalsRefreshContext) {
  return get().activeProjectId === context.projectId && openSessionRequestSequence === context.sessionGeneration;
}

async function refreshAgentTreeForContext(
  get: () => WorkbenchState,
  set: (partial: Partial<WorkbenchState>) => void,
  context: AgentTreeRefreshContext
) {
  if (!isActiveAgentTreeRefreshContext(get, context)) {
    return;
  }
  try {
    const agents = agentForestFromTreeResponse(await exagentClient.agentTree(context.projectId, context.threadId));
    if (isActiveAgentTreeRefreshContext(get, context)) {
      set({ agents: applyAgentTokenUsageMap(agents, get().tokenUsageByThreadId) });
    }
  } catch {
    // Keep the local event-derived tree when the app-server projection is unavailable.
  }
}

async function refreshApprovalsForContext(
  get: () => WorkbenchState,
  set: (partial: Partial<WorkbenchState>) => void,
  context: ApprovalsRefreshContext
) {
  if (!isActiveApprovalsRefreshContext(get, context)) {
    return;
  }
  set({ approvalsStatus: "loading", approvalsError: null });
  try {
    const response = await exagentClient.listApprovals(context.projectId);
    if (!isActiveApprovalsRefreshContext(get, context)) {
      return;
    }
    const approvalIds = new Set(response.approvals.map((item) => item.approval_id));
    set({
      pendingApprovals: response.approvals,
      selectedApprovalIds: filterSelectedApprovalIds(get().selectedApprovalIds, approvalIds),
      approvalsStatus: "ready",
      approvalsError: null
    });
  } catch (error) {
    if (isActiveApprovalsRefreshContext(get, context)) {
      set({ approvalsStatus: "error", approvalsError: errorMessage(error) });
    }
  }
}

export function __resetWorkbenchStoreRuntimeForTests() {
  cancelPendingAgentTreeRefresh();
  cancelPendingApprovalsRefresh();
  branchCompareRequestSequence = 0;
}

async function persistDraftGoal(
  projectId: string,
  threadId: string,
  draftGoal: DraftThreadGoal,
  set: (partial: Partial<WorkbenchState>) => void
) {
  try {
    const response = await exagentClient.setThreadGoal(projectId, threadId, {
      objective: draftGoal.objective,
      status: "active",
      tokenBudget: draftGoal.token_budget,
      clearTokenBudget: draftGoal.token_budget === null,
      mode: draftGoal.mode
    });
    set({
      currentGoal: response.goal,
      currentGoalMode: response.mode,
      draftGoal: null,
      goalEditorOpen: false,
      error: null
    });
    return true;
  } catch (error) {
    set({
      draftGoal,
      goalEditorOpen: false,
      error: errorMessage(error)
    });
    return false;
  }
}

function resetSelectedAgentThread(
  get: () => WorkbenchState,
  set: (partial: Partial<WorkbenchState>) => void
) {
  openAgentThreadRequestSequence += 1;
  get().selectedAgentUnlisten?.();
  set({
    selectedAgentThreadId: null,
    selectedAgentView: null,
    selectedAgentUnlisten: null,
    selectedAgentAppliedEventIds: new Set()
  });
}

function findAgentNode(nodes: AgentNode[], threadId: string): AgentNode | null {
  for (const node of nodes) {
    if (node.threadId === threadId) {
      return node;
    }
    const child = findAgentNode(node.children, threadId);
    if (child) {
      return child;
    }
  }
  return null;
}

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

async function refreshProjectSelection(
  get: () => WorkbenchState,
  set: (partial: Partial<WorkbenchState>) => void,
  preferredProjectId?: string | null
) {
  openSessionRequestSequence += 1;
  resetSelectedAgentThread(get, set);
  get().eventUnlisten?.();
  const projects = await exagentClient.listProjects();
  const targetProject =
    (preferredProjectId ? projects.find((project) => project.id === preferredProjectId) : null) ??
    projects[0] ??
    null;

  if (!targetProject) {
    set({
      projects: [],
      sessions: [],
      activeProjectId: null,
      activeSessionId: null,
      activeTurnId: null,
      transcript: [],
      currentGoal: null,
      currentGoalMode: "standard",
      draftGoal: null,
      goalEditorOpen: false,
      events: [],
      changedFiles: [],
      cwd: "No project selected",
      eventUnlisten: null,
      appliedRuntimeEventIds: new Set(),
      compareThreadId: null,
      compareView: null,
      loading: false,
      error: null
    });
    return;
  }

  const threads = get().search
    ? await exagentClient.listThreads(targetProject.id, false, get().search)
    : await exagentClient.reindexProject(targetProject.id);
  const targetSessionId = threads[0]?.id ?? null;
  set({
    projects: projects.map((project) => projectRecordToSummary(project, project.id === targetProject.id)),
    sessions: threads.map(exagentClient.threadRecordToSession),
    activeProjectId: targetProject.id,
    activeSessionId: targetSessionId,
    activeTurnId: null,
    transcript: [],
    currentGoal: null,
    currentGoalMode: "standard",
    draftGoal: null,
    goalEditorOpen: false,
    events: [],
    changedFiles: [],
    cwd: targetProject.path,
    eventUnlisten: null,
    appliedRuntimeEventIds: new Set(),
    compareThreadId: null,
    compareView: null,
    loading: false,
    error: null
  });

  if (targetSessionId) {
    await get().openSession(targetSessionId);
  }
}

function projectRecordToSummary(
  project: { id: string; name: string; path: string; archived_at?: number | null; pinned?: boolean },
  active: boolean
): ProjectSummary {
  return {
    id: project.id,
    name: project.name,
    path: project.path,
    active,
    pinned: project.pinned ?? false,
    archived: project.archived_at !== null && project.archived_at !== undefined
  };
}

function threadViewToTranscript(thread: ThreadView): TranscriptMessage[] {
  return thread.turns.flatMap((turn) => turnItemsToTranscript(thread.id, turn));
}

function postForkTranscript(thread: ThreadView, forkPointTurnId: string): TranscriptMessage[] {
  const forkPointIndex = thread.turns.findIndex((turn) => turn.id === forkPointTurnId);
  const divergentTurns = forkPointIndex >= 0 ? thread.turns.slice(forkPointIndex + 1) : thread.turns;
  return divergentTurns.flatMap((turn) => turnItemsToTranscript(thread.id, turn));
}

function compareViewReferencesThread(compareView: BranchCompareView | null, threadId: string): boolean {
  return compareView?.parentThreadId === threadId || compareView?.childThreadId === threadId;
}

function sharedTurnCountForFork(
  parentThread: ThreadView,
  childThread: ThreadView,
  forkPointTurnId: string
): number {
  const childForkPointIndex = childThread.turns.findIndex((turn) => turn.id === forkPointTurnId);
  if (childForkPointIndex >= 0) {
    return childForkPointIndex + 1;
  }
  const parentForkPointIndex = parentThread.turns.findIndex((turn) => turn.id === forkPointTurnId);
  return parentForkPointIndex >= 0 ? parentForkPointIndex + 1 : 0;
}

function threadViewEventIds(thread: ThreadView): Set<string> {
  const ids = new Set<string>();
  thread.turns.forEach((turn) => {
    turn.items.forEach((item) => {
      const eventId = threadItemEventId(item);
      if (eventId) {
        ids.add(eventId);
      }
    });
  });
  return ids;
}

function threadViewHasFailedTurn(thread: ThreadView): boolean {
  return thread.turns.some((turn) => turn.status === "failed");
}

function activeTurnIdFromThread(thread: ThreadView): string | null {
  if (thread.active_turn?.id) {
    return thread.active_turn.id;
  }
  const activeTurn = thread.turns.find((turn) => isActiveTurnStatus(turn.status));
  return activeTurn?.id ?? null;
}

function isActiveTurnStatus(status: string | undefined): boolean {
  return (
    status === "running" ||
    status === "waiting_approval" ||
    status === "waiting_user_input" ||
    status === "started" ||
    status === "in_progress"
  );
}

function applyActiveTurnEvents(
  activeTurnId: string | null,
  events: BackendRuntimeEvent[],
  threadId: string
): string | null {
  return events.reduce((current, event) => {
    if (event.thread_id !== threadId) {
      return current;
    }
    return applyActiveTurnEvent(current, event);
  }, activeTurnId);
}

function applyActiveTurnEvent(
  activeTurnId: string | null,
  event: BackendRuntimeEvent
): string | null {
  switch (event.kind.type) {
    case "turn_started":
    case "approval_requested":
    case "tool_invocation_waiting_approval":
    case "user_input_requested":
    case "tool_invocation_waiting_user_input":
      return event.turn_id ?? activeTurnId;
    case "turn_completed":
    case "turn_interrupted":
    case "runtime_error":
      if (!event.turn_id || event.turn_id === activeTurnId) {
        return null;
      }
      return activeTurnId;
    default:
      return activeTurnId;
  }
}

function threadItemEventId(item: ThreadItem): string | null {
  switch (item.type) {
    case "assistant_message":
    case "reasoning":
    case "tool_result":
    case "exec_output":
    case "approval_requested":
    case "approval_decision":
    case "user_input_requested":
    case "user_input_resolved":
    case "runtime_error":
    case "goal_report":
      return item.event_id ?? null;
    default:
      return null;
  }
}

function turnItemsToTranscript(threadId: string, turn: ThreadView["turns"][number]): TranscriptMessage[] {
  const hasToolInvocation = turn.items.some((item) => item.type === "tool_invocation");
  return turn.items.reduce<TranscriptMessage[]>((messages, item, index) => {
    const message = threadItemToTranscript(threadId, turn.id, turn.status, item, index, hasToolInvocation);
    return message ? appendTranscriptMessage(messages, message) : messages;
  }, []);
}

function threadItemToTranscript(
  threadId: string,
  turnId: string,
  turnStatus: string,
  item: ThreadItem,
  index: number,
  turnHasToolInvocation: boolean
): TranscriptMessage | null {
  const id = threadItemEventId(item) ?? `${threadId}-${turnId}-${item.type}-${index}`;
  switch (item.type) {
    case "user_message":
      return {
        id,
        role: "user",
        body: item.text,
        input: item.input ?? [],
        timestamp: "history",
        threadId,
        turnId,
        turnStatus
      };
    case "assistant_message":
      return {
        id,
        role: "assistant",
        body: item.text ?? "",
        timestamp: "history",
        threadId,
        turnId,
        turnStatus
      };
    case "reasoning":
      return {
        id,
        role: "reasoning",
        title: "Reasoning",
        body: reasoningBody(item.summary, item.content),
        timestamp: "history",
        threadId,
        turnId,
        turnStatus
      };
    case "tool_result":
      if (turnHasToolInvocation) {
        return null;
      }
      return {
        id,
        role: "tool",
        title: item.name,
        body: "Tool completed.",
        timestamp: "history",
        status: "info",
        threadId,
        turnId,
        turnStatus,
        toolName: item.name,
        toolStatus: "completed"
      };
    case "tool_invocation":
      return {
        id: `tool-${item.invocation_id}`,
        role: "tool",
        title: toolInvocationTitle(item),
        body: toolInvocationBody(item),
        timestamp: "history",
        status: toolInvocationTone(item),
        threadId,
        turnId,
        turnStatus,
        invocationId: item.invocation_id,
        toolCallId: item.tool_call_id ?? undefined,
        toolName: item.tool_name ?? undefined,
        approvalId: item.approval_id ?? undefined,
        requestId: item.request_id ?? undefined,
        toolStatus: normalizeToolInvocationStatus(item.status),
        mutating: item.mutating ?? undefined
      };
    case "exec_output":
      return null;
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
        turnStatus,
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
        turnId,
        turnStatus,
        approvalId: item.approval_id ?? undefined
      };
    case "user_input_requested":
      return {
        id,
        role: "tool",
        title: "Question for you",
        body: item.questions.map((question) => question.question).join("\n"),
        timestamp: "history",
        status: item.status === "pending" ? "warning" : "info",
        threadId,
        turnId,
        turnStatus,
        requestId: item.request_id,
        toolName: item.tool_name,
        toolStatus: item.status === "pending" ? "waiting_user_input" : "completed",
        questions: item.questions
      };
    case "user_input_resolved":
      return {
        id,
        role: "tool",
        title: item.dismissed ? "Question dismissed" : "Question answered",
        body: item.dismissed ? "User dismissed the question." : "User answered the question.",
        timestamp: "history",
        status: item.dismissed ? "warning" : "success",
        threadId,
        turnId,
        turnStatus,
        requestId: item.request_id,
        toolStatus: "completed"
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
        turnId,
        turnStatus
      };
    case "goal_report":
      return goalReportToTranscript(id, threadId, turnId, turnStatus, "history", item.report);
    case "subagent_spawn":
    case "subagent_close":
    case "inter_agent_message":
    case "compaction_written":
      return null;
  }
}

function goalReportToTranscript(
  id: string,
  threadId: string,
  turnId: string | undefined,
  turnStatus: string | undefined,
  timestamp: string,
  report: ThreadGoalReport
): TranscriptMessage {
  return {
    id,
    role: "goal_report",
    title: `Goal ${goalStatusLabel(report.final_status)}`,
    body: report.summary,
    timestamp,
    status: goalReportTone(report.final_status),
    threadId,
    turnId,
    turnStatus,
    goalReport: report
  };
}

function normalizeToolInvocationStatus(status: string): ToolInvocationTranscriptStatus {
  switch (status) {
    case "waiting_approval":
      return "waiting_approval";
    case "waiting_user_input":
      return "waiting_user_input";
    case "completed":
    case "approved":
      return "completed";
    case "failed":
      return "failed";
    case "cancelled":
    case "denied":
      return "cancelled";
    case "started":
    case "running":
    default:
      return "running";
  }
}

function toolInvocationTitle(item: Extract<ThreadItem, { type: "tool_invocation" }>) {
  if (item.status === "waiting_approval") {
    return "Waiting for approval";
  }
  if (item.status === "waiting_user_input") {
    return "Waiting for user input";
  }
  if (item.status === "approved") {
    return item.tool_name ?? "Approval approved";
  }
  if (item.status === "denied") {
    return item.tool_name ?? "Approval denied";
  }
  return item.tool_name ?? "Tool";
}

function toolInvocationBody(item: Extract<ThreadItem, { type: "tool_invocation" }>) {
  if (item.output_preview) {
    return item.output_preview;
  }
  if (item.message) {
    return item.message;
  }
  if (item.reason) {
    return item.reason;
  }
  if (item.status === "completed") {
    return "Tool completed.";
  }
  if (item.status === "approved") {
    return "Approval approved.";
  }
  if (item.status === "denied") {
    return "Approval denied.";
  }
  if (item.status === "waiting_user_input") {
    return "Waiting for the user to answer.";
  }
  if (item.status === "failed") {
    return "Tool failed.";
  }
  if (item.status === "cancelled") {
    return "Tool cancelled.";
  }
  return "Tool started.";
}

function toolInvocationTone(item: Extract<ThreadItem, { type: "tool_invocation" }>): TranscriptMessage["status"] {
  switch (item.status) {
    case "waiting_approval":
    case "waiting_user_input":
    case "cancelled":
      return "warning";
    case "completed":
    case "approved":
      return "success";
    case "failed":
    case "denied":
      return "danger";
    default:
      return "info";
  }
}

function applyTranscriptEvents(
  transcript: TranscriptMessage[],
  events: BackendRuntimeEvent[],
  representedEventIds = new Set<string>()
): TranscriptMessage[] {
  return events.reduce(
    (messages, event) => applyTranscriptEvent(messages, event, representedEventIds),
    transcript
  );
}

function applyGoalRuntimeEvents(goal: ThreadGoal | null, events: BackendRuntimeEvent[]): ThreadGoal | null {
  return events.reduce((currentGoal, event) => applyGoalRuntimeEvent(currentGoal, event), goal);
}

function applyGoalModeRuntimeEvents(mode: ThreadGoalMode, events: BackendRuntimeEvent[]): ThreadGoalMode {
  return events.reduce((currentMode, event) => applyGoalModeRuntimeEvent(currentMode, event), mode);
}

function applyTokenUsageEvents(
  current: Record<string, ThreadTokenUsage>,
  events: BackendRuntimeEvent[]
): Record<string, ThreadTokenUsage> {
  let next = current;

  for (const event of events) {
    if (event.kind.type !== "token_count" || !event.kind.info) {
      continue;
    }
    if (next === current) {
      next = { ...current };
    }
    next[event.thread_id] = {
      threadId: event.thread_id,
      total: event.kind.info.total_token_usage,
      last: event.kind.info.last_token_usage,
      modelContextWindow: event.kind.info.model_context_window ?? null
    };
  }

  return next;
}

function applyAgentTokenUsageMap(
  agents: AgentNode[],
  tokenUsageByThreadId: Record<string, ThreadTokenUsage>
): AgentNode[] {
  if (Object.keys(tokenUsageByThreadId).length === 0 || agents.length === 0) {
    return agents;
  }

  let changed = false;

  const visit = (node: AgentNode): AgentNode => {
    let childrenChanged = false;
    const children = node.children.map((child) => {
      const nextChild = visit(child);
      if (nextChild !== child) {
        childrenChanged = true;
      }
      return nextChild;
    });
    const tokenUsage = tokenUsageByThreadId[node.threadId];
    const tokensUsed = tokenUsage ? tokenUsage.total.total_tokens : node.tokensUsed;

    if (!childrenChanged && tokensUsed === node.tokensUsed) {
      return node;
    }

    changed = true;
    return {
      ...node,
      tokensUsed,
      children: childrenChanged ? children : node.children
    };
  };

  const nextAgents = agents.map(visit);
  return changed ? nextAgents : agents;
}

function applyGoalRuntimeEvent(goal: ThreadGoal | null, event: BackendRuntimeEvent): ThreadGoal | null {
  switch (event.kind.type) {
    case "thread_goal_updated":
      return visibleCurrentGoal(event.kind.goal);
    case "thread_goal_cleared":
      return null;
    case "thread_goal_report":
      return event.kind.report.final_status === "complete" ? null : goal;
    default:
      return goal;
  }
}

function applyGoalModeRuntimeEvent(mode: ThreadGoalMode, event: BackendRuntimeEvent): ThreadGoalMode {
  switch (event.kind.type) {
    case "thread_goal_mode_updated":
      return event.kind.mode;
    case "thread_goal_cleared":
      return "standard";
    case "thread_goal_report":
      return event.kind.report.final_status === "complete" ? "standard" : mode;
    default:
      return mode;
  }
}

function visibleCurrentGoal(goal: ThreadGoal | null): ThreadGoal | null {
  if (goal?.status === "complete") {
    return null;
  }
  return goal;
}

function applyTranscriptEvent(
  transcript: TranscriptMessage[],
  event: BackendRuntimeEvent,
  representedEventIds?: Set<string>
): TranscriptMessage[] {
  if (representedEventIds?.has(event.event_id)) {
    return transcript;
  }
  const nextTranscript = applyTurnStatusTranscriptEvent(transcript, event) ?? transcript;
  const streamingTranscript = applyStreamingTranscriptEvent(nextTranscript, event);
  if (streamingTranscript) {
    return streamingTranscript;
  }
  const toolMessage = runtimeEventToToolInvocationTranscript(event);
  if (toolMessage) {
    return upsertToolInvocationMessage(nextTranscript, toolMessage);
  }

  const rawTranscriptMessage = runtimeEventToTranscript(event);
  if (!rawTranscriptMessage) {
    return nextTranscript;
  }
  const transcriptMessage = withKnownTurnStatus(nextTranscript, rawTranscriptMessage);
  const finalizedStreamingTranscript = finalizeStreamingTranscriptMessage(nextTranscript, transcriptMessage);
  if (finalizedStreamingTranscript) {
    return finalizedStreamingTranscript;
  }
  if (nextTranscript.some((message) => message.id === transcriptMessage.id)) {
    return nextTranscript;
  }
  return [...nextTranscript, transcriptMessage];
}

function applyTurnStatusTranscriptEvent(
  transcript: TranscriptMessage[],
  event: BackendRuntimeEvent
): TranscriptMessage[] | null {
  const turnStatus = transcriptTurnStatusFromEvent(event);
  if (!turnStatus || !event.turn_id) {
    return null;
  }

  let changed = false;
  const next = transcript.map((message) => {
    if (
      message.threadId !== event.thread_id ||
      message.turnId !== event.turn_id ||
      message.turnStatus === turnStatus
    ) {
      return message;
    }
    changed = true;
    return { ...message, turnStatus };
  });

  return changed ? next : null;
}

function transcriptTurnStatusFromEvent(event: BackendRuntimeEvent): string | null {
  switch (event.kind.type) {
    case "turn_started":
      return "running";
    case "approval_requested":
    case "tool_invocation_waiting_approval":
    case "user_input_requested":
    case "tool_invocation_waiting_user_input":
      return "waiting_approval";
    case "turn_completed":
      return "completed";
    case "turn_interrupted":
      return "interrupted";
    case "runtime_error":
      return "failed";
    default:
      return null;
  }
}

function applyStreamingTranscriptEvent(
  transcript: TranscriptMessage[],
  event: BackendRuntimeEvent
): TranscriptMessage[] | null {
  switch (event.kind.type) {
    case "assistant_text_delta":
      return upsertStreamingTranscriptMessage(transcript, event, "assistant", event.kind.delta);
    case "reasoning_delta":
      return upsertStreamingTranscriptMessage(transcript, event, "reasoning", event.kind.delta);
    default:
      return null;
  }
}

function upsertStreamingTranscriptMessage(
  transcript: TranscriptMessage[],
  event: BackendRuntimeEvent,
  role: Extract<TranscriptMessage["role"], "assistant" | "reasoning">,
  delta: string
): TranscriptMessage[] {
  if (!delta) {
    return transcript;
  }

  const index = findStreamingTranscriptIndex(transcript, event.thread_id, event.turn_id ?? undefined, role);
  if (index === -1) {
    return [
      ...transcript,
      {
        id: streamingTranscriptId(event, role),
        role,
        title: role === "reasoning" ? "Reasoning" : undefined,
        body: delta,
        timestamp: "now",
        threadId: event.thread_id,
        turnId: event.turn_id ?? undefined
      }
    ];
  }

  const next = [...transcript];
  const current = next[index];
  next[index] = {
    ...current,
    body: `${current.body}${delta}`,
    timestamp: "now"
  };
  return next;
}

function finalizeStreamingTranscriptMessage(
  transcript: TranscriptMessage[],
  finalized: TranscriptMessage
): TranscriptMessage[] | null {
  if (finalized.role !== "assistant" && finalized.role !== "reasoning") {
    return null;
  }
  const index = findStreamingTranscriptIndex(transcript, finalized.threadId, finalized.turnId, finalized.role);
  if (index === -1) {
    return null;
  }
  const next = [...transcript];
  next[index] = finalized;
  return next;
}

function withKnownTurnStatus(
  transcript: TranscriptMessage[],
  message: TranscriptMessage
): TranscriptMessage {
  if (message.turnStatus || !message.threadId || !message.turnId) {
    return message;
  }
  const known = transcript.find(
    (item) =>
      item.threadId === message.threadId &&
      item.turnId === message.turnId &&
      typeof item.turnStatus === "string" &&
      item.turnStatus.length > 0
  );
  return known?.turnStatus ? { ...message, turnStatus: known.turnStatus } : message;
}

function findStreamingTranscriptIndex(
  transcript: TranscriptMessage[],
  threadId: string | undefined,
  turnId: string | undefined,
  role: Extract<TranscriptMessage["role"], "assistant" | "reasoning">
) {
  const prefix = streamingTranscriptPrefix(role);
  for (let index = transcript.length - 1; index >= 0; index -= 1) {
    const message = transcript[index];
    if (
      message.id.startsWith(prefix) &&
      message.role === role &&
      message.threadId === threadId &&
      message.turnId === turnId
    ) {
      return index;
    }
  }
  return -1;
}

function streamingTranscriptId(
  event: BackendRuntimeEvent,
  role: Extract<TranscriptMessage["role"], "assistant" | "reasoning">
) {
  return `${streamingTranscriptPrefix(role)}${event.thread_id}-${event.turn_id ?? "unscoped"}`;
}

function streamingTranscriptPrefix(role: Extract<TranscriptMessage["role"], "assistant" | "reasoning">) {
  return `stream-${role}-`;
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
    case "reasoning":
      return {
        ...base,
        role: "reasoning",
        title: "Reasoning",
        body: reasoningBody(event.kind.summary ?? [], event.kind.content ?? [])
      };
    case "exec_output":
      return null;
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
    case "thread_goal_report":
      return goalReportToTranscript(
        event.event_id,
        event.thread_id,
        event.turn_id ?? undefined,
        undefined,
        "now",
        event.kind.report
      );
    default:
      return null;
  }
}

function goalStatusLabel(status: ThreadGoal["status"]) {
  return status.replace(/_/g, " ");
}

function goalReportTone(status: ThreadGoal["status"]): TranscriptMessage["status"] {
  switch (status) {
    case "complete":
      return "success";
    case "blocked":
    case "usage_limited":
    case "budget_limited":
      return "warning";
    default:
      return "info";
  }
}

function reasoningBody(summary?: string[] | null, content?: string[] | null): string {
  return [...(summary ?? []), ...(content ?? [])]
    .map((part) => part.trim())
    .filter(Boolean)
    .join("\n\n");
}

function runtimeEventToToolInvocationTranscript(event: BackendRuntimeEvent): TranscriptMessage | null {
  const base = {
    id: `tool-${toolInvocationId(event)}`,
    role: "tool" as const,
    timestamp: "now",
    threadId: event.thread_id,
    turnId: event.turn_id ?? undefined
  };

  switch (event.kind.type) {
    case "tool_result":
      if (isReviewRequiredToolResult(event.kind.result.status)) {
        const waitingForUserInput = event.kind.result.tool_name === "ask_user";
        return {
          ...base,
          title: waitingForUserInput ? "Waiting for user input" : "Waiting for approval",
          body: event.kind.result.content || toolResultFallbackBody(event.kind.result.status),
          status: "warning",
          toolCallId: event.kind.result.tool_call_id,
          toolName: event.kind.result.tool_name,
          toolStatus: waitingForUserInput ? "waiting_user_input" : "waiting_approval"
        };
      }
      return {
        ...base,
        title: event.kind.result.tool_name,
        body: event.kind.result.content || toolResultFallbackBody(event.kind.result.status),
        status: toolResultTone(event.kind.result.status),
        toolCallId: event.kind.result.tool_call_id,
        toolName: event.kind.result.tool_name,
        toolStatus: toolResultTranscriptStatus(event.kind.result.status)
      };
    case "tool_invocation_started":
      return {
        ...base,
        title: event.kind.tool_name,
        body: "Tool started.",
        status: "info",
        invocationId: event.kind.invocation_id,
        toolCallId: event.kind.tool_call_id,
        toolName: event.kind.tool_name,
        toolStatus: "running",
        mutating: event.kind.mutating
      };
    case "tool_invocation_waiting_approval":
      return {
        ...base,
        title: "Waiting for approval",
        body: event.kind.reason,
        status: "warning",
        invocationId: event.kind.invocation_id,
        approvalId: event.kind.approval_id,
        toolStatus: "waiting_approval"
      };
    case "tool_invocation_waiting_user_input":
      return {
        ...base,
        title: "Waiting for user input",
        body: event.kind.reason,
        status: "warning",
        invocationId: event.kind.invocation_id,
        requestId: event.kind.request_id,
        toolStatus: "waiting_user_input"
      };
    case "tool_invocation_output_delta":
      return {
        ...base,
        title: event.kind.stream,
        body: event.kind.chunk,
        status: event.kind.stream === "stderr" ? "warning" : "info",
        invocationId: event.kind.invocation_id,
        toolStatus: "running"
      };
    case "tool_invocation_completed":
      return {
        ...base,
        title: event.kind.tool_name,
        body: `Tool completed with ${event.kind.status}.`,
        status: event.kind.status === "success" ? "success" : "info",
        invocationId: event.kind.invocation_id,
        toolCallId: event.kind.tool_call_id,
        toolName: event.kind.tool_name,
        toolStatus: "completed"
      };
    case "tool_invocation_failed":
      return {
        ...base,
        title: event.kind.tool_name,
        body: event.kind.message,
        status: "danger",
        invocationId: event.kind.invocation_id,
        toolCallId: event.kind.tool_call_id,
        toolName: event.kind.tool_name,
        toolStatus: "failed"
      };
    case "tool_invocation_cancelled":
      return {
        ...base,
        title: event.kind.tool_name,
        body: event.kind.reason,
        status: "warning",
        invocationId: event.kind.invocation_id,
        toolCallId: event.kind.tool_call_id,
        toolName: event.kind.tool_name,
        toolStatus: "cancelled"
      };
    case "approval_decision":
      return {
        ...base,
        title: `Approval ${event.kind.status}`,
        body: event.kind.note ?? `Approval ${event.kind.status}.`,
        status: event.kind.status === "approved" ? "success" : "danger",
        approvalId: event.kind.approval_id,
        toolStatus: event.kind.status === "approved" ? "completed" : "cancelled"
      };
    case "user_input_requested":
      return {
        ...base,
        title: "Question for you",
        body: event.kind.questions.map((question) => question.question).join("\n"),
        status: "warning",
        requestId: event.kind.request_id,
        toolName: event.kind.tool_name,
        toolStatus: "waiting_user_input",
        questions: event.kind.questions
      };
    case "user_input_resolved":
      return {
        ...base,
        title: event.kind.dismissed ? "Question dismissed" : "Question answered",
        body: event.kind.dismissed ? "User dismissed the question." : "User answered the question.",
        status: event.kind.dismissed ? "warning" : "success",
        requestId: event.kind.request_id,
        toolStatus: "completed"
      };
    default:
      return null;
  }
}

function upsertToolInvocationMessage(
  transcript: TranscriptMessage[],
  update: TranscriptMessage
): TranscriptMessage[] {
  if (!update.invocationId && !update.toolCallId && !update.approvalId && !update.requestId) {
    return [...transcript, update];
  }

  const existingIndex = matchingToolMessageIndex(transcript, update);
  if (existingIndex === -1) {
    return [...transcript, update];
  }

  const next = [...transcript];
  const current = next[existingIndex];
  next[existingIndex] = mergeToolInvocationMessage(current, update);
  return next;
}

function appendTranscriptMessage(
  transcript: TranscriptMessage[],
  message: TranscriptMessage
): TranscriptMessage[] {
  if (message.invocationId || message.requestId) {
    return upsertToolInvocationMessage(transcript, message);
  }
  return [...transcript, message];
}

function mergeToolInvocationMessage(
  current: TranscriptMessage,
  update: TranscriptMessage
): TranscriptMessage {
  const isDelta = update.toolStatus === "running" && !update.toolName && Boolean(update.body);
  const keepTerminalStatus = isTerminalToolStatus(current.toolStatus) && update.toolStatus === "running";
  const body = isDelta ? appendOutputChunk(current.body, update.body) : mergedToolBody(current, update);
  const toolStatus = strongestToolStatus(current.toolStatus, update.toolStatus);

  return {
    ...current,
    ...update,
    title: mergedToolTitle(current, update),
    body,
    status: keepTerminalStatus ? current.status : update.status ?? current.status,
    toolStatus,
    toolCallId: update.toolCallId ?? current.toolCallId,
    toolName: update.toolName ?? current.toolName,
    approvalId: update.approvalId ?? current.approvalId,
    requestId: update.requestId ?? current.requestId,
    mutating: update.mutating ?? current.mutating
  };
}

function matchingToolMessageIndex(transcript: TranscriptMessage[], update: TranscriptMessage) {
  const matchers = [
    update.invocationId
      ? (message: TranscriptMessage) => message.invocationId === update.invocationId
      : null,
    update.toolCallId
      ? (message: TranscriptMessage) => message.toolCallId === update.toolCallId
      : null,
    update.approvalId
      ? (message: TranscriptMessage) => message.approvalId === update.approvalId
      : null,
    update.requestId
      ? (message: TranscriptMessage) => message.requestId === update.requestId
      : null,
    update.toolStatus === "waiting_approval" && update.approvalId
      ? (message: TranscriptMessage) =>
          message.toolStatus === "waiting_approval" && !message.approvalId && !message.invocationId
      : null,
    update.toolStatus === "waiting_user_input" && update.requestId
      ? (message: TranscriptMessage) =>
          message.toolStatus === "waiting_user_input" && !message.requestId && !message.invocationId
      : null
  ].filter((matcher): matcher is (message: TranscriptMessage) => boolean => Boolean(matcher));

  for (const matcher of matchers) {
    const index = transcript.findIndex((message) => transcriptMessageInScope(message, update) && matcher(message));
    if (index !== -1) {
      return index;
    }
  }
  return -1;
}

function transcriptMessageInScope(message: TranscriptMessage, update: TranscriptMessage) {
  if (update.threadId && message.threadId !== update.threadId) {
    return false;
  }
  if (update.turnId && message.turnId !== update.turnId) {
    return false;
  }
  return true;
}

function isTerminalToolStatus(status: ToolInvocationTranscriptStatus | undefined) {
  return status === "completed" || status === "failed" || status === "cancelled";
}

function mergedToolTitle(current: TranscriptMessage, update: TranscriptMessage) {
  if (update.toolStatus === "waiting_approval" || update.toolStatus === "waiting_user_input") {
    return update.title ?? current.title;
  }
  if ((update.approvalId || update.requestId) && update.toolStatus) {
    return update.title ?? current.title;
  }
  return update.toolName ?? current.toolName ?? update.title ?? current.title;
}

function mergedToolBody(current: TranscriptMessage, update: TranscriptMessage) {
  if (
    (current.toolStatus === "waiting_approval" && update.toolStatus === "waiting_approval") ||
    (current.toolStatus === "waiting_user_input" && update.toolStatus === "waiting_user_input")
  ) {
    if (update.approvalId || update.requestId || update.invocationId) {
      return update.body || current.body;
    }
    return current.body || update.body;
  }
  if (
    (update.approvalId || update.requestId) &&
    update.toolStatus &&
    update.toolStatus !== "waiting_approval" &&
    update.toolStatus !== "waiting_user_input"
  ) {
    return update.body || current.body;
  }
  if (
    update.toolStatus === "completed" &&
    current.body &&
    current.body !== "Tool started." &&
    !current.body.startsWith("Tool completed")
  ) {
    return current.body;
  }
  return update.body || current.body;
}

function appendOutputChunk(currentBody: string, chunk: string) {
  if (!currentBody || currentBody === "Tool started.") {
    return chunk;
  }
  return `${currentBody}${chunk}`;
}

function strongestToolStatus(
  current: ToolInvocationTranscriptStatus | undefined,
  update: ToolInvocationTranscriptStatus | undefined
): ToolInvocationTranscriptStatus | undefined {
  if (!current) {
    return update;
  }
  if (!update) {
    return current;
  }
  const order: Record<ToolInvocationTranscriptStatus, number> = {
    running: 0,
    waiting_approval: 1,
    waiting_user_input: 1,
    completed: 2,
    cancelled: 3,
    failed: 4
  };
  return order[update] >= order[current] ? update : current;
}

function toolResultTranscriptStatus(status: string): ToolInvocationTranscriptStatus {
  if (isReviewRequiredToolResult(status)) {
    return "waiting_approval";
  }
  if (status === "error" || status === "failed" || status === "failure") {
    return "failed";
  }
  return "completed";
}

function toolResultTone(status: string): TranscriptMessage["status"] {
  if (isReviewRequiredToolResult(status)) {
    return "warning";
  }
  return toolResultTranscriptStatus(status) === "failed" ? "danger" : "success";
}

function toolResultFallbackBody(status: string) {
  if (isReviewRequiredToolResult(status)) {
    return "Waiting for approval.";
  }
  return toolResultTranscriptStatus(status) === "failed" ? "Tool failed." : "Tool completed.";
}

function isReviewRequiredToolResult(status: string) {
  return status === "review_required";
}

function toolInvocationId(event: BackendRuntimeEvent) {
  switch (event.kind.type) {
    case "tool_invocation_started":
    case "tool_invocation_waiting_approval":
    case "tool_invocation_waiting_user_input":
    case "tool_invocation_output_delta":
    case "tool_invocation_completed":
    case "tool_invocation_failed":
    case "tool_invocation_cancelled":
      return event.kind.invocation_id;
    default:
      return event.event_id;
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

function runtimeEventsToInspector(events: BackendRuntimeEvent[]): RuntimeEvent[] {
  return events.filter(shouldShowInspectorEvent).map(runtimeEventToInspector);
}

function shouldShowInspectorEvent(event: BackendRuntimeEvent) {
  switch (event.kind.type) {
    case "assistant_text_delta":
    case "reasoning_delta":
    case "subagent_spawned":
    case "subagent_closed":
    case "inter_agent_message_sent":
      return false;
    default:
      return true;
  }
}

function eventDetail(event: BackendRuntimeEvent) {
  switch (event.kind.type) {
    case "assistant_turn":
      return event.kind.turn.text ?? "Assistant turn";
    case "tool_result":
      return event.kind.result.tool_name;
    case "tool_invocation_started":
      return event.kind.tool_name;
    case "tool_invocation_waiting_approval":
      return event.kind.reason;
    case "tool_invocation_waiting_user_input":
      return event.kind.reason;
    case "tool_invocation_output_delta":
      return `${event.kind.stream} #${event.kind.sequence}`;
    case "tool_invocation_completed":
      return `${event.kind.tool_name}: ${event.kind.status}`;
    case "tool_invocation_failed":
      return event.kind.message;
    case "tool_invocation_cancelled":
      return event.kind.reason;
    case "approval_requested":
      return event.kind.reason;
    case "approval_decision":
      return event.kind.note ?? event.kind.status;
    case "user_input_requested":
      return event.kind.questions.map((question) => question.question).join(" ");
    case "user_input_resolved":
      return event.kind.dismissed ? "dismissed" : "answered";
    case "review_submitted":
      return event.kind.findings ?? event.kind.verdict;
    case "open_question_recorded":
      return event.kind.question;
    case "open_question_resolved":
      return event.kind.answer ?? "resolved";
    case "compaction_written":
      return event.kind.summary.summary;
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
    case "tool_invocation_waiting_approval":
    case "user_input_requested":
    case "tool_invocation_waiting_user_input":
      return "warning";
    case "approval_decision":
      return event.kind.status === "approved" ? "success" : "danger";
    case "user_input_resolved":
      return event.kind.dismissed ? "warning" : "success";
    case "tool_invocation_completed":
      return "success";
    case "tool_invocation_failed":
      return "danger";
    case "tool_invocation_cancelled":
      return "warning";
    case "turn_completed":
      return "success";
    default:
      return "info";
  }
}

function rootAgentStatus(status: SessionSummary["status"] | undefined): AgentRunStatus {
  if (status === "awaiting_approval") {
    return "waiting_approval";
  }
  return status === "running" ? "running" : "idle";
}

function sessionStatusFromThreadStatus(status: string | undefined): SessionSummary["status"] {
  switch (status) {
    case "running":
    case "idle":
    case "failed":
      return status;
    case "waiting_approval":
      return "awaiting_approval";
    default:
      return "idle";
  }
}

function updateSessionStatus(
  sessions: SessionSummary[],
  threadId: string,
  status: SessionSummary["status"]
): SessionSummary[] {
  return sessions.map((session) =>
    session.id === threadId && session.status !== status ? { ...session, status } : session
  );
}

function markSessionStatus(
  set: (partial: Partial<WorkbenchState>) => void,
  get: () => WorkbenchState,
  threadId: string,
  status: SessionSummary["status"]
) {
  const current = get();
  const sessions = updateSessionStatus(current.sessions, threadId, status);
  const agents =
    current.activeSessionId === threadId
      ? buildAgentForest(threadId, rootAgentStatus(status), flattenAgentForest(current.agents))
      : current.agents;
  set({
    sessions,
    agents,
    activeTurnId:
      current.activeSessionId === threadId && status !== "running" && status !== "awaiting_approval"
        ? null
        : current.activeTurnId
  });
}

function isNoActiveTurnError(error: unknown) {
  return errorMessage(error).toLowerCase().includes("no active turn");
}

function shouldRefreshAgentTreeAfterEvent(event: BackendRuntimeEvent) {
  switch (event.kind.type) {
    case "subagent_spawned":
    case "subagent_closed":
    case "inter_agent_message_sent":
    case "turn_started":
    case "turn_completed":
    case "turn_interrupted":
    case "runtime_error":
    case "tool_invocation_started":
    case "tool_invocation_completed":
    case "tool_invocation_failed":
    case "tool_invocation_cancelled":
    case "tool_invocation_waiting_approval":
    case "tool_invocation_waiting_user_input":
    case "approval_requested":
    case "approval_decision":
    case "user_input_requested":
    case "user_input_resolved":
    case "token_count":
      return true;
    default:
      return false;
  }
}

function shouldRefreshApprovalsAfterEvent(event: BackendRuntimeEvent) {
  switch (event.kind.type) {
    case "approval_requested":
    case "approval_decision":
    case "tool_invocation_waiting_approval":
    case "user_input_requested":
    case "user_input_resolved":
    case "tool_invocation_waiting_user_input":
    case "open_question_recorded":
    case "open_question_resolved":
      return true;
    default:
      return false;
  }
}

function statusFromEvent(event: BackendRuntimeEvent, current: SessionSummary["status"]) {
  switch (event.kind.type) {
    case "turn_started":
      return "running";
    case "approval_requested":
    case "tool_invocation_waiting_approval":
    case "user_input_requested":
    case "tool_invocation_waiting_user_input":
      return "awaiting_approval";
    case "tool_invocation_failed":
    case "runtime_error":
      return "failed";
    case "approval_decision":
    case "user_input_resolved":
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

async function submitInboxApprovalDecision(
  get: () => WorkbenchState,
  set: (partial: Partial<WorkbenchState>) => void,
  item: PendingApprovalItem,
  decision: "approved" | "denied",
  note: string
) {
  const context = currentApprovalsRefreshContext(get);
  if (!context) {
    return;
  }

  set({ approvalsStatus: "submitting", approvalsError: null, approvalActionStatus: null });
  try {
    await exagentClient.submitApprovalDecision(
      context.projectId,
      item.thread_id,
      undefined,
      item.approval_id,
      decision,
      note
    );
    if (!isActiveApprovalsRefreshContext(get, context)) {
      return;
    }
    set({
      pendingApprovals: get().pendingApprovals.filter((pending) => pending.approval_id !== item.approval_id),
      selectedApprovalIds: removeApprovalId(get().selectedApprovalIds, item.approval_id),
      approvalsStatus: "ready",
      approvalsError: null,
      approvalActionStatus: { type: "approval_decision", approval_id: item.approval_id, decision }
    });
    await refreshApprovalsForContext(get, set, context);
  } catch (error) {
    if (!isActiveApprovalsRefreshContext(get, context)) {
      return;
    }
    const message = errorMessage(error);
    set({ approvalsStatus: "error", approvalsError: message, approvalActionStatus: null });
  }
}

async function submitInboxOpenQuestionResolution(
  get: () => WorkbenchState,
  set: (partial: Partial<WorkbenchState>) => void,
  item: PendingApprovalItem,
  answer: string
) {
  const context = currentApprovalsRefreshContext(get);
  if (!context) {
    return;
  }

  set({ approvalsStatus: "submitting", approvalsError: null, approvalActionStatus: null });
  try {
    await exagentClient.resolveOpenQuestion(
      context.projectId,
      item.thread_id,
      item.approval_id,
      answer.trim() ? answer.trim() : null
    );
    if (!isActiveApprovalsRefreshContext(get, context)) {
      return;
    }
    set({
      pendingApprovals: get().pendingApprovals.filter((pending) => pending.approval_id !== item.approval_id),
      selectedApprovalIds: removeApprovalId(get().selectedApprovalIds, item.approval_id),
      approvalsStatus: "ready",
      approvalsError: null,
      approvalActionStatus: { type: "open_question_resolved", approval_id: item.approval_id }
    });
    await refreshApprovalsForContext(get, set, context);
  } catch (error) {
    if (!isActiveApprovalsRefreshContext(get, context)) {
      return;
    }
    const message = errorMessage(error);
    set({ approvalsStatus: "error", approvalsError: message, approvalActionStatus: null });
  }
}

function removeApprovalId(selectedApprovalIds: Set<string>, approvalId: string) {
  const next = new Set(selectedApprovalIds);
  next.delete(approvalId);
  return next;
}

function filterSelectedApprovalIds(selectedApprovalIds: Set<string>, availableApprovalIds: Set<string>) {
  const next = new Set<string>();
  for (const approvalId of selectedApprovalIds) {
    if (availableApprovalIds.has(approvalId)) {
      next.add(approvalId);
    }
  }
  return next;
}

type SelectionState = Pick<
  WorkbenchState,
  "providerSettings" | "runtimeSettings" | "activeProviderId" | "selectedModel" | "selectedThinkingMode"
>;

function normalizeWorkbenchSelection(state: Partial<SelectionState>) {
  const activeProviderId = activeProviderIdFromSettings(state.providerSettings, state.activeProviderId);
  const requestedModel = normalizeModelRef(state.selectedModel ?? null, activeProviderId);
  const selectedModel = isProviderConfigurationRequired(state.providerSettings)
    ? null
    : selectableModelRef(requestedModel, state.providerSettings) ??
      providerConfigModelRef(state.providerSettings) ??
      modelRefFromString(state.runtimeSettings?.default_model, activeProviderId);
  const selectedThinkingMode = normalizeThinkingModeForModel(
    state.selectedThinkingMode ?? null,
    selectedModel,
    state.providerSettings
  );

  return {
    activeProviderId,
    selectedModel,
    selectedThinkingMode
  };
}

function normalizeActiveWorkbenchSelection(state: WorkbenchState) {
  const activeThreadSelection = state.activeSessionId
    ? state.selectionByThreadId[state.activeSessionId]
    : null;
  return normalizeWorkbenchSelection({
    ...state,
    selectedModel: activeThreadSelection ? activeThreadSelection.selectedModel : state.selectedModel,
    selectedThinkingMode: activeThreadSelection
      ? activeThreadSelection.selectedThinkingMode
      : state.selectedThinkingMode
  });
}

function selectionForThread(
  state: WorkbenchState,
  threadId: string,
  fallbackSelection?: ThreadSelection | null
) {
  const scopedSelection = state.selectionByThreadId[threadId] ?? null;
  const requestedSelection = scopedSelection ?? fallbackSelection ?? null;
  return normalizeWorkbenchSelection({
    providerSettings: state.providerSettings,
    runtimeSettings: state.runtimeSettings,
    activeProviderId: state.activeProviderId,
    selectedModel: requestedSelection?.selectedModel ?? null,
    selectedThinkingMode: requestedSelection?.selectedThinkingMode ?? null
  });
}

function threadViewSelection(thread: ThreadView): ThreadSelection | null {
  return thread.model
    ? {
        selectedModel: thread.model,
        selectedThinkingMode: thread.thinking_mode ?? null
      }
    : null;
}

function selectableModelRef(
  model: ModelRef | null,
  settings?: ProviderSettingsResponse | null
): ModelRef | null {
  if (!model) {
    return null;
  }
  if (!settings) {
    return model;
  }
  if (isProviderConfigurationRequired(settings)) {
    return null;
  }
  return modelIsSelectable(model, settings) ? model : null;
}

function modelIsSelectable(model: ModelRef, settings: ProviderSettingsResponse) {
  const providerId = model.provider_id;
  const modelId = model.model_id;
  if (settings.config.provider_id === providerId && settings.config.model === modelId) {
    return true;
  }
  if (settings.connected_provider?.id === providerId && settings.connected_provider.model === modelId) {
    return true;
  }
  if (
    settings.configured_providers.some(
      (provider) => provider.provider_id === providerId && provider.model === modelId
    )
  ) {
    return true;
  }

  const providerOptions = settings.model_options.filter((option) => option.provider_id === providerId);
  if (providerOptions.some((option) => option.id === modelId)) {
    return true;
  }

  const providerIsConfigured =
    settings.config.provider_id === providerId ||
    settings.connected_provider?.id === providerId ||
    settings.configured_providers.some((provider) => provider.provider_id === providerId);
  return providerIsConfigured && providerOptions.length === 0;
}

function withThreadSelection(
  selectionByThreadId: Record<string, ThreadSelection>,
  threadId: string | null,
  selection: ReturnType<typeof normalizeWorkbenchSelection>
) {
  if (!threadId) {
    return selectionByThreadId;
  }
  return {
    ...selectionByThreadId,
    [threadId]: {
      selectedModel: selection.selectedModel,
      selectedThinkingMode: selection.selectedThinkingMode
    }
  };
}

function activeProviderIdFromSettings(settings?: ProviderSettingsResponse | null, fallback?: string | null) {
  return settings?.config.provider_id?.trim() || settings?.active_provider_id?.trim() || fallback?.trim() || DEFAULT_PROVIDER_ID;
}

function providerConfigModelRef(settings?: ProviderSettingsResponse | null): ModelRef | null {
  if (!settings) {
    return null;
  }
  return modelRefFromString(settings.config.model, activeProviderIdFromSettings(settings, null));
}

function isProviderConfigurationRequired(settings?: ProviderSettingsResponse | null) {
  const missingRequiredCredential = Boolean(
    settings?.config.auth_required && !settings.config.has_api_key && !settings.config.has_credential
  );
  return Boolean(
    settings &&
      settings.connected_provider === null &&
      settings.configured_providers.length === 0 &&
      settings.model_options.length === 0 &&
      missingRequiredCredential
  );
}

function normalizeThinkingModeForModel(
  mode: ThinkingMode | null,
  model: ModelRef | null,
  settings?: ProviderSettingsResponse | null
): Exclude<ThinkingMode, "auto"> | null {
  const normalizedMode = mode === "auto" ? null : mode;
  if (normalizedMode === null || !model) {
    return null;
  }

  const thinking = settings?.model_options.find(
    (option) => option.provider_id === model.provider_id && option.id === model.model_id
  )?.capabilities.thinking;
  if (!thinking?.supported) {
    return null;
  }

  return thinking.modes.includes(normalizedMode) ? normalizedMode : null;
}

function turnThinkingOverride(state: SelectionState, normalized: ReturnType<typeof normalizeWorkbenchSelection>) {
  const requestedMode = state.selectedThinkingMode === "auto" ? null : state.selectedThinkingMode ?? null;
  const thinkingMode = normalized.selectedThinkingMode;
  const thinking = selectedModelThinkingCapability(normalized, state);
  const defaultMode = state.runtimeSettings?.default_thinking_mode === "auto" ? null : state.runtimeSettings?.default_thinking_mode ?? null;
  const explicitModeWasRejected = requestedMode !== null && thinkingMode === null && thinking !== null;
  const inheritedDefaultIsUnsupported =
    requestedMode === null && thinking !== null && defaultMode !== null && !thinking.modes.includes(defaultMode);
  const clearThinkingMode = explicitModeWasRejected || inheritedDefaultIsUnsupported;

  return {
    thinkingMode,
    clearThinkingMode
  };
}

function selectedModelThinkingCapability(normalized: ReturnType<typeof normalizeWorkbenchSelection>, state: SelectionState) {
  const model = normalized.selectedModel;
  if (!model) {
    return null;
  }
  return (
    state.providerSettings?.model_options.find(
      (option) => option.provider_id === model.provider_id && option.id === model.model_id
    )?.capabilities.thinking ?? null
  );
}

function selectedModelInputModalities(
  normalized: ReturnType<typeof normalizeWorkbenchSelection>,
  state: SelectionState
): InputModality[] {
  const model = normalized.selectedModel;
  if (!model) {
    return ["text", "image"];
  }

  const configured = state.providerSettings?.model_options.find(
    (option) => option.provider_id === model.provider_id && option.id === model.model_id
  )?.capabilities.input_modalities;
  if (configured?.length) {
    return configured;
  }

  return isKnownTextOnlyModel(model) ? ["text"] : ["text", "image"];
}

function isKnownTextOnlyModel(model: ModelRef) {
  const providerId = model.provider_id.toLowerCase();
  const modelId = model.model_id.toLowerCase();
  return providerId === "deepseek" || modelId.startsWith("embedding") || modelId.includes("/embedding");
}
