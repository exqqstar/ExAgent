import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { exagentClient } from "@/api/exagentClient";
import { __resetWorkbenchStoreRuntimeForTests, useWorkbenchStore } from "@/stores/workbenchStore";
import type {
  AgentTreeResponse,
  BackendRuntimeEvent,
  ProviderModelView,
  ProviderSettingsResponse,
  RuntimeSettingsResponse,
  ThreadReadResponse,
  WorkflowRunView
} from "@/types";

describe("workbenchStore runtime events", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    __resetWorkbenchStoreRuntimeForTests();
    useWorkbenchStore.setState(useWorkbenchStore.getInitialState(), true);
  });

  afterEach(() => {
    __resetWorkbenchStoreRuntimeForTests();
    vi.useRealTimers();
  });

  it("keeps child thread events out of the root transcript until the agent viewer applies them", () => {
    const childEvent: BackendRuntimeEvent = {
      event_id: "event-child-answer",
      thread_id: "thread-child",
      turn_id: "turn-child",
      kind: {
        type: "assistant_turn",
        turn: {
          text: "Child answer",
          tool_calls: []
        }
      }
    };

    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project",
      activeSessionId: "thread-root",
      transcript: [
        {
          id: "root-message",
          role: "assistant",
          body: "Root answer",
          timestamp: "history",
          threadId: "thread-root",
          turnId: "turn-root"
        }
      ],
      selectedAgentThreadId: "thread-child",
      selectedAgentView: {
        threadId: "thread-child",
        transcript: [],
        events: [],
        loading: false,
        error: null
      },
      selectedAgentAppliedEventIds: new Set()
    });

    useWorkbenchStore.getState().applyRuntimeEvents([childEvent]);

    expect(useWorkbenchStore.getState().transcript).toEqual([
      expect.objectContaining({ body: "Root answer", threadId: "thread-root" })
    ]);
    expect(useWorkbenchStore.getState().selectedAgentView?.transcript).toEqual([]);

    useWorkbenchStore.getState().applySelectedAgentRuntimeEvents([childEvent]);

    expect(useWorkbenchStore.getState().transcript).toEqual([
      expect.objectContaining({ body: "Root answer", threadId: "thread-root" })
    ]);
    expect(useWorkbenchStore.getState().selectedAgentView?.transcript).toEqual([
      expect.objectContaining({ body: "Child answer", threadId: "thread-child" })
    ]);
  });

  it("copies known completed turn status onto a late assistant transcript message", () => {
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      activeSessionId: "thread-late-final",
      transcript: [
        {
          id: "user-late-final",
          role: "user",
          body: "Prompt",
          timestamp: "now",
          threadId: "thread-late-final",
          turnId: "turn-late-final",
          turnStatus: "completed"
        }
      ],
      loading: false
    });

    useWorkbenchStore.getState().applyRuntimeEvent({
      event_id: "evt-late-assistant",
      thread_id: "thread-late-final",
      turn_id: "turn-late-final",
      kind: {
        type: "assistant_turn",
        turn: {
          text: "Late final answer",
          tool_calls: []
        }
      }
    });

    expect(useWorkbenchStore.getState().transcript).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          id: "evt-late-assistant",
          role: "assistant",
          body: "Late final answer",
          turnStatus: "completed"
        })
      ])
    );
  });

  it("stores token count events by thread id", () => {
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project",
      activeSessionId: "thread-root",
      tokenUsageByThreadId: {}
    });

    const tokenEvent: BackendRuntimeEvent = {
      event_id: "evt-token-root",
      thread_id: "thread-root",
      turn_id: "turn-1",
      kind: {
        type: "token_count",
        info: {
          total_token_usage: {
            input_tokens: 142000,
            cached_input_tokens: 28000,
            output_tokens: 31200,
            reasoning_output_tokens: 13200,
            total_tokens: 186400
          },
          last_token_usage: {
            input_tokens: 52000,
            cached_input_tokens: 8000,
            output_tokens: 6200,
            reasoning_output_tokens: 1200,
            total_tokens: 59400
          },
          model_context_window: 400000
        }
      }
    };

    useWorkbenchStore.getState().applyRuntimeEvents([tokenEvent]);

    expect(useWorkbenchStore.getState().tokenUsageByThreadId["thread-root"]).toEqual({
      threadId: "thread-root",
      total: {
        input_tokens: 142000,
        cached_input_tokens: 28000,
        output_tokens: 31200,
        reasoning_output_tokens: 13200,
        total_tokens: 186400
      },
      last: {
        input_tokens: 52000,
        cached_input_tokens: 8000,
        output_tokens: 6200,
        reasoning_output_tokens: 1200,
        total_tokens: 59400
      },
      modelContextWindow: 400000
    });
  });

  it("does not apply stale approval decision results after switching projects", async () => {
    const decision = createDeferred<unknown>();
    vi.spyOn(exagentClient, "submitApprovalDecision").mockReturnValue(decision.promise as Promise<any>);
    vi.spyOn(exagentClient, "listApprovals").mockResolvedValue({ approvals: [] });
    const originalApproval = {
      thread_id: "thread-a",
      approval_id: "approval-a",
      kind: "command" as const,
      summary: "Run A",
      detail: "cargo test",
      goal_id: null,
      requested_at_ms: 1,
      checkpoint_id: null
    };
    const nextProjectApproval = {
      thread_id: "thread-b",
      approval_id: "approval-b",
      kind: "command" as const,
      summary: "Run B",
      detail: "npm test",
      goal_id: null,
      requested_at_ms: 2,
      checkpoint_id: null
    };
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project-a",
      activeSessionId: "thread-a",
      pendingApprovals: [originalApproval],
      approvalsStatus: "ready",
      approvalActionStatus: null
    });

    const approve = useWorkbenchStore.getState().approveInboxApproval(originalApproval);
    useWorkbenchStore.setState({
      activeProjectId: "project-b",
      activeSessionId: "thread-b",
      pendingApprovals: [nextProjectApproval],
      approvalsStatus: "ready",
      approvalActionStatus: null
    });
    decision.resolve({});
    await approve;
    await Promise.resolve();

    expect(useWorkbenchStore.getState().pendingApprovals).toEqual([nextProjectApproval]);
    expect(useWorkbenchStore.getState().approvalActionStatus).toBeNull();
    expect(exagentClient.listApprovals).not.toHaveBeenCalled();
  });

  it("does not refresh approvals or set errors for stale transcript approval completions", async () => {
    const decision = createDeferred<unknown>();
    vi.spyOn(exagentClient, "submitApprovalDecision").mockReturnValue(decision.promise as Promise<any>);
    vi.spyOn(exagentClient, "listApprovals").mockResolvedValue({ approvals: [] });
    const nextProjectApproval = {
      thread_id: "thread-b",
      approval_id: "approval-b",
      kind: "command" as const,
      summary: "Run B",
      detail: "npm test",
      goal_id: null,
      requested_at_ms: 2,
      checkpoint_id: null
    };
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project-a",
      activeSessionId: "thread-a",
      pendingApprovals: [],
      approvalsStatus: "ready",
      error: null
    });

    const submit = useWorkbenchStore.getState().submitApproval(
      {
        id: "message-approval-a",
        role: "approval",
        body: "Approve command",
        timestamp: "now",
        threadId: "thread-a",
        turnId: "turn-a",
        approvalId: "approval-a"
      },
      "approved"
    );
    useWorkbenchStore.setState({
      activeProjectId: "project-b",
      activeSessionId: "thread-b",
      pendingApprovals: [nextProjectApproval],
      approvalsStatus: "ready",
      error: null
    });
    decision.reject(new Error("old approval failed"));
    await submit;
    await Promise.resolve();

    expect(exagentClient.submitApprovalDecision).toHaveBeenCalledWith(
      "project-a",
      "thread-a",
      "turn-a",
      "approval-a",
      "approved",
      "desktop approved"
    );
    expect(useWorkbenchStore.getState().pendingApprovals).toEqual([nextProjectApproval]);
    expect(useWorkbenchStore.getState().error).toBeNull();
    expect(exagentClient.listApprovals).not.toHaveBeenCalled();
  });

  it("still restores the captured checkpoint when rollback context becomes stale after denial", async () => {
    const denial = createDeferred<unknown>();
    const restore = createDeferred<unknown>();
    vi.spyOn(exagentClient, "submitApprovalDecision").mockReturnValue(denial.promise as Promise<any>);
    vi.spyOn(exagentClient, "restoreCheckpoint").mockReturnValue(restore.promise as Promise<any>);
    vi.spyOn(exagentClient, "listApprovals").mockResolvedValue({ approvals: [] });
    const originalApproval = {
      thread_id: "thread-a",
      approval_id: "approval-a",
      kind: "command" as const,
      summary: "Run A",
      detail: "cargo test",
      goal_id: null,
      requested_at_ms: 1,
      checkpoint_id: "checkpoint-a"
    };
    const nextProjectApproval = {
      thread_id: "thread-b",
      approval_id: "approval-b",
      kind: "command" as const,
      summary: "Run B",
      detail: "npm test",
      goal_id: null,
      requested_at_ms: 2,
      checkpoint_id: null
    };
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project-a",
      activeSessionId: "thread-a",
      pendingApprovals: [originalApproval],
      approvalsStatus: "ready",
      approvalActionStatus: null
    });

    const rollback = useWorkbenchStore.getState().rejectAndRollbackApproval(originalApproval);
    denial.resolve({});
    useWorkbenchStore.setState({
      activeProjectId: "project-b",
      activeSessionId: "thread-b",
      pendingApprovals: [nextProjectApproval],
      approvalsStatus: "ready",
      approvalActionStatus: null
    });
    await Promise.resolve();

    expect(exagentClient.restoreCheckpoint).toHaveBeenCalledWith("project-a", "checkpoint-a");
    restore.resolve({});
    await rollback;

    expect(useWorkbenchStore.getState().pendingApprovals).toEqual([nextProjectApproval]);
    expect(useWorkbenchStore.getState().approvalActionStatus).toBeNull();
    expect(exagentClient.listApprovals).not.toHaveBeenCalled();
  });

  it("does not clear existing thread usage when a token count event has null info", () => {
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project",
      activeSessionId: "thread-root",
      tokenUsageByThreadId: {
        "thread-root": {
          threadId: "thread-root",
          total: {
            input_tokens: 123,
            cached_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 123
          },
          last: {
            input_tokens: 123,
            cached_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 123
          },
          modelContextWindow: null
        }
      }
    });

    const tokenEvent: BackendRuntimeEvent = {
      event_id: "evt-token-null",
      thread_id: "thread-root",
      kind: {
        type: "token_count",
        info: null
      }
    };

    useWorkbenchStore.getState().applyRuntimeEvents([tokenEvent]);

    expect(useWorkbenchStore.getState().tokenUsageByThreadId["thread-root"]?.total.total_tokens).toBe(123);
  });

  it("stores selected agent token count events without changing the root transcript", () => {
    const childTokenEvent: BackendRuntimeEvent = {
      event_id: "evt-token-child",
      thread_id: "thread-child",
      turn_id: "turn-child",
      kind: {
        type: "token_count",
        info: {
          total_token_usage: {
            input_tokens: 1200,
            cached_input_tokens: 200,
            output_tokens: 300,
            reasoning_output_tokens: 100,
            total_tokens: 1600
          },
          last_token_usage: {
            input_tokens: 800,
            cached_input_tokens: 100,
            output_tokens: 200,
            reasoning_output_tokens: 50,
            total_tokens: 1050
          },
          model_context_window: null
        }
      }
    };

    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project",
      activeSessionId: "thread-root",
      transcript: [
        {
          id: "root-message",
          role: "assistant",
          body: "Root answer",
          timestamp: "history",
          threadId: "thread-root",
          turnId: "turn-root"
        }
      ],
      selectedAgentThreadId: "thread-child",
      selectedAgentView: {
        threadId: "thread-child",
        transcript: [],
        events: [],
        loading: false,
        error: null
      },
      selectedAgentAppliedEventIds: new Set(),
      tokenUsageByThreadId: {}
    });

    useWorkbenchStore.getState().applySelectedAgentRuntimeEvents([childTokenEvent]);

    expect(useWorkbenchStore.getState().transcript).toEqual([
      expect.objectContaining({ body: "Root answer", threadId: "thread-root" })
    ]);
    expect(useWorkbenchStore.getState().tokenUsageByThreadId["thread-child"]?.total.total_tokens).toBe(1600);
  });

  it("updates the selected agent tree token count from live token count events", () => {
    const childTokenEvent = tokenCountEvent("evt-token-child-live", 2800, "thread-child");
    mountSelectedChildAgentSession(1200);

    useWorkbenchStore.getState().applySelectedAgentRuntimeEvents([childTokenEvent]);

    const child = useWorkbenchStore.getState().agents[0]?.children[0];
    expect(useWorkbenchStore.getState().tokenUsageByThreadId["thread-child"]?.total.total_tokens).toBe(2800);
    expect(child?.tokensUsed).toBe(2800);
  });

  it("stores the workflow run after starting a workflow", async () => {
    const workflowRun = workflowRunView("workflow_run_thread-workflow", "running");
    vi.spyOn(exagentClient, "startWorkflow").mockResolvedValue({
      run_id: workflowRun.run_id,
      thread_id: workflowRun.thread_id,
      status: "queued"
    });
    vi.spyOn(exagentClient, "readWorkflow").mockResolvedValue({ run: workflowRun });
    vi.spyOn(exagentClient, "reindexProject").mockResolvedValue([
      {
        id: "thread-workflow",
        project_id: "project",
        rollout_path: "/workspace/project/.exagent/threads/thread-workflow/rollout.jsonl",
        user_title: null,
        fallback_title: "Deep research: test",
        preview: "",
        title_source: "fallback",
        status: "running",
        updated_at: 1,
        created_at: 1,
        last_opened_at: null,
        pinned: false,
        archived_at: null,
        fork_parent_thread_id: null,
        fork_point_turn_id: null
      }
    ]);
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue(threadReadResponse("thread-workflow"));
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(vi.fn());
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "thread-workflow",
      events: []
    });
    vi.spyOn(exagentClient, "agentTree").mockResolvedValue(agentTreeResponse("thread-workflow"));
    vi.spyOn(exagentClient, "listApprovals").mockResolvedValue({ approvals: [] });

    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project",
      projects: [
        {
          id: "project",
          name: "Project",
          path: "/workspace/project",
          active: true
        }
      ]
    });

    await useWorkbenchStore.getState().startWorkflow("project", {
      templateId: "deep-research",
      presetId: "quick",
      question: "Research workflow visibility"
    });

    expect(exagentClient.readWorkflow).toHaveBeenCalledWith("project", "workflow_run_thread-workflow");
    expect(useWorkbenchStore.getState().activeWorkflowRun).toEqual(workflowRun);
  });

  it("keeps live agent token counts when a delayed agent tree refresh is older", async () => {
    vi.useFakeTimers();
    vi.spyOn(exagentClient, "agentTree").mockResolvedValue(agentTreeWithChildTokens(1200));
    const childTokenEvent = tokenCountEvent("evt-token-child-before-refresh", 2800, "thread-child");
    mountSelectedChildAgentSession(1200);

    useWorkbenchStore.getState().applySelectedAgentRuntimeEvents([childTokenEvent]);
    expect(useWorkbenchStore.getState().agents[0]?.children[0]?.tokensUsed).toBe(2800);

    await vi.advanceTimersByTimeAsync(300);

    expect(exagentClient.agentTree).toHaveBeenCalledWith("project", "thread-root");
    expect(useWorkbenchStore.getState().agents[0]?.children[0]?.tokensUsed).toBe(2800);
  });

  it("keeps selected models scoped to their active thread session", async () => {
    vi.spyOn(exagentClient, "resumeThread").mockImplementation(async (_projectId, threadId) =>
      threadReadResponse(threadId)
    );
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(vi.fn());
    vi.spyOn(exagentClient, "replayEvents").mockImplementation(async (_projectId, threadId) => ({
      thread_id: threadId,
      events: []
    }));
    vi.spyOn(exagentClient, "agentTree").mockImplementation(async (_projectId, threadId) =>
      agentTreeResponse(threadId)
    );
    vi.spyOn(exagentClient, "listApprovals").mockResolvedValue({ approvals: [] });
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "thread-a",
      turn: {
        id: "turn-a",
        status: "in_progress",
        items: []
      }
    });

    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project",
      activeSessionId: "thread-a",
      activeProviderId: "openai",
      sessions: [
        {
          id: "thread-a",
          projectId: "project",
          title: "Thread A",
          updatedAt: "now",
          status: "idle"
        },
        {
          id: "thread-b",
          projectId: "project",
          title: "Thread B",
          updatedAt: "now",
          status: "idle"
        }
      ],
      providerSettings: openAiProviderSettings(),
      runtimeSettings: runtimeSettings(),
      selectedModel: { provider_id: "openai", model_id: "gpt-5.5" },
      selectedThinkingMode: null
    });

    useWorkbenchStore.getState().setSelectedModel({ provider_id: "openai", model_id: "gpt-4.1-mini" });
    await useWorkbenchStore.getState().openSession("thread-b");
    useWorkbenchStore.getState().setSelectedModel({ provider_id: "openai", model_id: "gpt-5.5" });
    await useWorkbenchStore.getState().openSession("thread-a");

    expect(useWorkbenchStore.getState().selectedModel).toEqual({
      provider_id: "openai",
      model_id: "gpt-4.1-mini"
    });

    useWorkbenchStore.getState().setComposerValue("Use the original model");
    await useWorkbenchStore.getState().sendPrompt();

    expect(startTurn).toHaveBeenCalledWith(
      "project",
      "thread-a",
      "Use the original model",
      expect.objectContaining({
        model: {
          provider_id: "openai",
          model_id: "gpt-4.1-mini"
        }
      })
    );
  });

  it("keeps the current transcript visible while opening another session", async () => {
    const pendingRead = createDeferred<ThreadReadResponse>();
    vi.spyOn(exagentClient, "resumeThread").mockReturnValue(pendingRead.promise);
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(vi.fn());
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "thread-b",
      events: []
    });
    vi.spyOn(exagentClient, "agentTree").mockResolvedValue(agentTreeResponse("thread-b"));
    vi.spyOn(exagentClient, "listApprovals").mockResolvedValue({ approvals: [] });

    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project",
      activeSessionId: "thread-a",
      sessions: [
        {
          id: "thread-a",
          projectId: "project",
          title: "Thread A",
          updatedAt: "now",
          status: "idle"
        },
        {
          id: "thread-b",
          projectId: "project",
          title: "Thread B",
          updatedAt: "now",
          status: "idle"
        }
      ],
      transcript: [
        {
          id: "thread-a-message",
          role: "assistant",
          body: "Thread A remains visible",
          timestamp: "history",
          threadId: "thread-a"
        }
      ]
    });

    const open = useWorkbenchStore.getState().openSession("thread-b");
    await Promise.resolve();

    expect(useWorkbenchStore.getState()).toMatchObject({
      activeSessionId: "thread-b",
      loading: true
    });
    expect(useWorkbenchStore.getState().transcript).toEqual([
      expect.objectContaining({ body: "Thread A remains visible" })
    ]);

    pendingRead.resolve(threadReadResponse("thread-b"));
    await open;

    expect(useWorkbenchStore.getState()).toMatchObject({
      activeSessionId: "thread-b",
      loading: false
    });
  });

  it("keeps the current view stable while preparing a personal draft session", async () => {
    const pendingProject = createDeferred<{
      id: string;
      name: string;
      path: string;
      archived_at: null;
      pinned: boolean;
    }>();
    vi.spyOn(exagentClient, "getOrCreatePersonalProject").mockReturnValue(pendingProject.promise);
    vi.spyOn(exagentClient, "listProjects").mockResolvedValue([
      {
        id: "project-personal",
        name: "Personal",
        path: "/tmp/personal",
        archived_at: null,
        pinned: false
      }
    ]);

    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project-personal",
      activeSessionId: "thread-a",
      projects: [
        {
          id: "project-personal",
          name: "Personal",
          path: "/tmp/personal",
          active: true
        }
      ],
      sessions: [
        {
          id: "thread-a",
          projectId: "project-personal",
          title: "Thread A",
          updatedAt: "now",
          status: "idle"
        }
      ],
      transcript: [
        {
          id: "thread-a-message",
          role: "assistant",
          body: "Current view remains visible",
          timestamp: "history",
          threadId: "thread-a"
        }
      ]
    });

    const start = useWorkbenchStore.getState().startPersonalSession();
    await Promise.resolve();

    expect(useWorkbenchStore.getState()).toMatchObject({
      loading: false,
      activeSessionId: "thread-a"
    });
    expect(useWorkbenchStore.getState().transcript).toEqual([
      expect.objectContaining({ body: "Current view remains visible" })
    ]);

    pendingProject.resolve({
      id: "project-personal",
      name: "Personal",
      path: "/tmp/personal",
      archived_at: null,
      pinned: false
    });
    await start;

    expect(useWorkbenchStore.getState()).toMatchObject({
      loading: false,
      activeProjectId: "project-personal",
      activeSessionId: null,
      transcript: []
    });
  });

  it("restores a reopened thread session model from the backend thread view", async () => {
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        ...threadReadResponse("thread-a").thread,
        model: {
          provider_id: "openai",
          model_id: "gpt-4.1-mini"
        },
        thinking_mode: null
      }
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(vi.fn());
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "thread-a",
      events: []
    });
    vi.spyOn(exagentClient, "agentTree").mockResolvedValue(agentTreeResponse("thread-a"));
    vi.spyOn(exagentClient, "listApprovals").mockResolvedValue({ approvals: [] });

    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project",
      activeSessionId: null,
      activeProviderId: "openai",
      sessions: [
        {
          id: "thread-a",
          projectId: "project",
          title: "Thread A",
          updatedAt: "now",
          status: "idle"
        }
      ],
      providerSettings: openAiProviderSettings(),
      runtimeSettings: runtimeSettings(),
      selectedModel: { provider_id: "openai", model_id: "gpt-5.5" },
      selectedThinkingMode: null,
      selectionByThreadId: {}
    });

    await useWorkbenchStore.getState().openSession("thread-a");

    expect(useWorkbenchStore.getState().selectedModel).toEqual({
      provider_id: "openai",
      model_id: "gpt-4.1-mini"
    });
  });

  it("falls back to the configured provider model when a reopened thread references an unavailable model", async () => {
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        ...threadReadResponse("thread-stale").thread,
        model: {
          provider_id: "removed-provider",
          model_id: "removed-model"
        },
        thinking_mode: null
      }
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(vi.fn());
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "thread-stale",
      events: []
    });
    vi.spyOn(exagentClient, "agentTree").mockResolvedValue(agentTreeResponse("thread-stale"));
    vi.spyOn(exagentClient, "listApprovals").mockResolvedValue({ approvals: [] });

    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project",
      activeSessionId: null,
      activeProviderId: "openai",
      sessions: [
        {
          id: "thread-stale",
          projectId: "project",
          title: "Thread Stale",
          updatedAt: "now",
          status: "idle"
        }
      ],
      providerSettings: openAiProviderSettings(),
      runtimeSettings: runtimeSettings(),
      selectedModel: { provider_id: "openai", model_id: "gpt-5.5" },
      selectedThinkingMode: null,
      selectionByThreadId: {}
    });

    await useWorkbenchStore.getState().openSession("thread-stale");

    expect(useWorkbenchStore.getState().selectedModel).toEqual({
      provider_id: "openai",
      model_id: "gpt-5.5"
    });
  });

  it("debounces agent tree refreshes for agent-relevant runtime event bursts", async () => {
    vi.useFakeTimers();
    const agentTree = vi.spyOn(exagentClient, "agentTree").mockResolvedValue({
      root: {
        thread_id: "thread-root",
        root_thread_id: "thread-root",
        depth: 0,
        agent_path: "root",
        status: "waiting_approval",
        current_tool: "run_command",
        tokens_used: 1800,
        children: []
      }
    });

    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project",
      activeSessionId: "thread-root",
      sessions: [
        {
          id: "thread-root",
          projectId: "project",
          title: "Root thread",
          updatedAt: "now",
          status: "running"
        }
      ],
      agents: [
        {
          threadId: "thread-root",
          parentThreadId: null,
          name: "Root agent",
          agentPath: "root",
          status: "running",
          task: "",
          lastActivity: null,
          currentTool: null,
          tokensUsed: null,
          isRoot: true,
          children: []
        }
      ]
    });

    useWorkbenchStore.getState().applyRuntimeEvent({
      event_id: "evt-child-token-count",
      thread_id: "thread-child",
      turn_id: "turn-child",
      kind: {
        type: "token_count",
        info: {
          total_token_usage: {
            input_tokens: 1800,
            cached_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 1800
          },
          last_token_usage: {
            input_tokens: 1800,
            cached_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 1800
          },
          model_context_window: null
        }
      }
    });
    useWorkbenchStore.getState().applyRuntimeEvent({
      event_id: "evt-spawn-child",
      thread_id: "thread-root",
      turn_id: "turn-root",
      kind: {
        type: "subagent_spawned",
        invocation_id: "inv_spawn",
        tool_call_id: "call_spawn",
        parent_thread_id: "thread-root",
        child_thread_id: "thread-child",
        task_name: "Worker",
        message_preview: "Check the branch"
      }
    });

    expect(agentTree).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(299);
    expect(agentTree).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(1);

    expect(agentTree).toHaveBeenCalledTimes(1);
    expect(agentTree).toHaveBeenCalledWith("project", "thread-root");
    expect(useWorkbenchStore.getState().agents[0]).toMatchObject({
      threadId: "thread-root",
      status: "waiting_approval",
      currentTool: "run_command",
      tokensUsed: 1800
    });
    expect(useWorkbenchStore.getState().transcript).toEqual([]);
  });

  it("ignores a pending agent tree refresh when the active session changes before the debounce fires", async () => {
    vi.useFakeTimers();
    const agentTree = vi.spyOn(exagentClient, "agentTree").mockResolvedValue(agentTreeResponse("thread-old"));

    mountRootSession("thread-old");

    useWorkbenchStore.getState().applyRuntimeEvent(turnStartedEvent("evt-old-turn", "thread-old"));
    useWorkbenchStore.setState({
      activeSessionId: "thread-new",
      agents: [rootAgent("thread-new", "running")]
    });

    await vi.advanceTimersByTimeAsync(300);

    expect(agentTree).not.toHaveBeenCalled();
    expect(useWorkbenchStore.getState().agents[0]).toMatchObject({
      threadId: "thread-new",
      status: "running"
    });
  });

  it("drops a stale in-flight agent tree response after the active session changes", async () => {
    vi.useFakeTimers();
    const staleTree = createDeferred<AgentTreeResponse>();
    const agentTree = vi.spyOn(exagentClient, "agentTree").mockReturnValue(staleTree.promise);

    mountRootSession("thread-old");

    useWorkbenchStore.getState().applyRuntimeEvent(turnStartedEvent("evt-old-inflight", "thread-old"));
    await vi.advanceTimersByTimeAsync(300);
    expect(agentTree).toHaveBeenCalledWith("project", "thread-old");

    useWorkbenchStore.setState({
      activeSessionId: "thread-new",
      agents: [rootAgent("thread-new", "running")]
    });
    staleTree.resolve(agentTreeResponse("thread-old", { status: "failed", currentTool: "run_command" }));
    await Promise.resolve();

    expect(useWorkbenchStore.getState().agents[0]).toMatchObject({
      threadId: "thread-new",
      status: "running",
      currentTool: null
    });
  });

  it("keeps a newer session agent tree refresh when stale open session cleanup runs", async () => {
    vi.useFakeTimers();
    const oldResume = createDeferred<ThreadReadResponse>();
    vi.spyOn(exagentClient, "resumeThread").mockImplementation(async (_projectId, threadId) => {
      if (threadId === "thread-old") {
        return oldResume.promise;
      }
      return threadReadResponse(threadId);
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(vi.fn());
    vi.spyOn(exagentClient, "replayEvents").mockImplementation(async (_projectId, threadId) => ({
      thread_id: threadId,
      events: []
    }));
    const agentTree = vi.spyOn(exagentClient, "agentTree").mockResolvedValue(
      agentTreeResponse("thread-new", { status: "waiting_approval", currentTool: "run_command" })
    );

    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project",
      activeSessionId: null,
      sessions: [
        {
          id: "thread-old",
          projectId: "project",
          title: "Old thread",
          updatedAt: "now",
          status: "running"
        },
        {
          id: "thread-new",
          projectId: "project",
          title: "New thread",
          updatedAt: "now",
          status: "running"
        }
      ]
    });

    const staleOpen = useWorkbenchStore.getState().openSession("thread-old");
    await Promise.resolve();

    await useWorkbenchStore.getState().openSession("thread-new");
    agentTree.mockClear();

    useWorkbenchStore.getState().applyRuntimeEvent(turnStartedEvent("evt-new-turn", "thread-new"));
    oldResume.resolve(threadReadResponse("thread-old"));
    await staleOpen;

    await vi.advanceTimersByTimeAsync(300);

    expect(agentTree).toHaveBeenCalledTimes(1);
    expect(agentTree).toHaveBeenCalledWith("project", "thread-new");
    expect(useWorkbenchStore.getState().agents[0]).toMatchObject({
      threadId: "thread-new",
      status: "waiting_approval",
      currentTool: "run_command"
    });
  });

  it("keeps replayed token usage when a buffered live event has the same event id", async () => {
    const staleBufferedEvent = tokenCountEvent("evt-token-duplicate", 100);
    const freshReplayEvent = tokenCountEvent("evt-token-duplicate", 200);

    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "thread-root",
        status: "running",
        goal_mode: "standard",
        active_turn: null,
        turns: []
      }
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockImplementation(async (_projectId, _threadId, onEvent) => {
      onEvent(staleBufferedEvent);
      return vi.fn();
    });
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "thread-root",
      events: [freshReplayEvent]
    });
    vi.spyOn(exagentClient, "agentTree").mockResolvedValue({
      root: {
        thread_id: "thread-root",
        root_thread_id: "thread-root",
        depth: 0,
        agent_path: "root",
        status: "running",
        children: []
      }
    });

    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project",
      activeSessionId: "thread-root",
      sessions: [
        {
          id: "thread-root",
          projectId: "project",
          title: "Root thread",
          updatedAt: "now",
          status: "running"
        }
      ],
      tokenUsageByThreadId: {}
    });

    await useWorkbenchStore.getState().openSession("thread-root");

    expect(useWorkbenchStore.getState().tokenUsageByThreadId["thread-root"]?.total.total_tokens).toBe(200);
  });

  it("compacts the active thread and applies replayed runtime events", async () => {
    const compactThread = vi.spyOn(exagentClient, "compactThread").mockResolvedValue({
      thread_id: "thread-root",
      latest_compaction: { summary: "manual compact summary" }
    });
    const replayEvents = vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "thread-root",
      events: [
        {
          event_id: "evt-compact",
          thread_id: "thread-root",
          kind: {
            type: "compaction_written",
            summary: { summary: "manual compact summary" }
          }
        },
        {
          event_id: "evt-assistant",
          thread_id: "thread-root",
          turn_id: "turn-1",
          kind: {
            type: "assistant_turn",
            turn: {
              text: "Replayed assistant text",
              tool_calls: []
            }
          }
        },
        tokenCountEvent("evt-token-root-after-compact", 300)
      ]
    });

    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project",
      activeSessionId: "thread-root",
      sessions: [
        {
          id: "thread-root",
          projectId: "project",
          title: "Root thread",
          updatedAt: "now",
          status: "idle"
        }
      ],
      error: "previous error"
    });

    await useWorkbenchStore.getState().compactActiveThread();

    expect(compactThread).toHaveBeenCalledWith("project", "thread-root");
    expect(replayEvents).toHaveBeenCalledWith("project", "thread-root", null);
    expect(useWorkbenchStore.getState().error).toBeNull();
    expect(useWorkbenchStore.getState().events).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          id: "evt-compact",
          label: "compaction written",
          detail: "manual compact summary"
        })
      ])
    );
    expect(useWorkbenchStore.getState().transcript).toEqual([
      expect.objectContaining({ id: "evt-assistant", body: "Replayed assistant text" })
    ]);
    expect(useWorkbenchStore.getState().tokenUsageByThreadId["thread-root"]?.total.total_tokens).toBe(300);
  });

  it("sets an error when active thread compaction fails", async () => {
    vi.spyOn(exagentClient, "compactThread").mockRejectedValue(new Error("manual compaction failed"));
    const replayEvents = vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "thread-root",
      events: []
    });

    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project",
      activeSessionId: "thread-root",
      events: []
    });

    await useWorkbenchStore.getState().compactActiveThread();

    expect(useWorkbenchStore.getState().error).toBe("manual compaction failed");
    expect(replayEvents).not.toHaveBeenCalled();
    expect(useWorkbenchStore.getState().events).toEqual([]);
  });
});

function tokenCountEvent(eventId: string, totalTokens: number, threadId = "thread-root"): BackendRuntimeEvent {
  return {
    event_id: eventId,
    thread_id: threadId,
    turn_id: "turn-1",
    kind: {
      type: "token_count",
      info: {
        total_token_usage: {
          input_tokens: totalTokens,
          cached_input_tokens: 0,
          output_tokens: 0,
          reasoning_output_tokens: 0,
          total_tokens: totalTokens
        },
        last_token_usage: {
          input_tokens: totalTokens,
          cached_input_tokens: 0,
          output_tokens: 0,
          reasoning_output_tokens: 0,
          total_tokens: totalTokens
        },
        model_context_window: null
      }
    }
  };
}

function mountSelectedChildAgentSession(childTokensUsed: number) {
  useWorkbenchStore.setState({
    ...useWorkbenchStore.getInitialState(),
    loading: false,
    activeProjectId: "project",
    activeSessionId: "thread-root",
    selectedAgentThreadId: "thread-child",
    selectedAgentView: {
      threadId: "thread-child",
      transcript: [],
      events: [],
      loading: false,
      error: null
    },
    selectedAgentAppliedEventIds: new Set(),
    agents: [
      {
        ...rootAgent("thread-root", "running"),
        children: [
          {
            threadId: "thread-child",
            parentThreadId: "thread-root",
            name: "worker",
            agentPath: "root/worker",
            status: "running",
            task: "inspect token usage",
            lastActivity: null,
            currentTool: null,
            tokensUsed: childTokensUsed,
            isRoot: false,
            children: []
          }
        ]
      }
    ],
    tokenUsageByThreadId: {}
  });
}

function mountRootSession(threadId: string) {
  useWorkbenchStore.setState({
    ...useWorkbenchStore.getInitialState(),
    loading: false,
    activeProjectId: "project",
    activeSessionId: threadId,
    sessions: [
      {
        id: threadId,
        projectId: "project",
        title: "Root thread",
        updatedAt: "now",
        status: "running"
      }
    ],
    agents: [rootAgent(threadId, "running")]
  });
}

function rootAgent(threadId: string, status: "running" | "waiting_approval" | "failed") {
  return {
    threadId,
    parentThreadId: null,
    name: "Root agent",
    agentPath: "root",
    status,
    task: "",
    lastActivity: null,
    currentTool: null,
    tokensUsed: null,
    isRoot: true,
    children: []
  };
}

function turnStartedEvent(eventId: string, threadId: string): BackendRuntimeEvent {
  return {
    event_id: eventId,
    thread_id: threadId,
    turn_id: "turn-1",
    kind: { type: "turn_started" }
  };
}

function threadReadResponse(threadId: string): ThreadReadResponse {
  return {
    thread: {
      id: threadId,
      status: "running",
      goal_mode: "standard",
      active_turn: null,
      turns: []
    }
  };
}

function workflowRunView(runId: string, status: WorkflowRunView["status"]): WorkflowRunView {
  return {
    run_id: runId,
    thread_id: runId.replace(/^workflow_run_/, ""),
    template_id: "deep-research",
    preset_id: "quick",
    label: "Deep research: test",
    status,
    phases: [
      {
        id: "search",
        label: "Search",
        status: "running",
        planned_count: 3,
        completed_count: 1,
        failed_count: 0,
        skipped_count: 0,
        updated_at_ms: 2
      }
    ],
    artifacts: [],
    stats: {
      agent_calls: 1,
      failed_agent_calls: 0,
      skipped_agent_calls: 0,
      total_artifacts: 0,
      elapsed_ms: 10,
      template_stats: {}
    },
    created_at_ms: 1,
    updated_at_ms: 2
  };
}

function agentTreeResponse(
  threadId: string,
  fields: { status?: "running" | "waiting_approval" | "failed"; currentTool?: string | null } = {}
): AgentTreeResponse {
  return {
    root: {
      thread_id: threadId,
      root_thread_id: threadId,
      depth: 0,
      agent_path: "root",
      status: fields.status ?? "running",
      current_tool: fields.currentTool ?? null,
      children: []
    }
  };
}

function agentTreeWithChildTokens(tokensUsed: number): AgentTreeResponse {
  return {
    root: {
      thread_id: "thread-root",
      root_thread_id: "thread-root",
      depth: 0,
      agent_path: "root",
      status: "running",
      children: [
        {
          thread_id: "thread-child",
          parent_thread_id: "thread-root",
          root_thread_id: "thread-root",
          depth: 1,
          agent_path: "root/worker",
          status: "running",
          tokens_used: tokensUsed,
          children: []
        }
      ]
    }
  };
}

function openAiProviderSettings(): ProviderSettingsResponse {
  return {
    providers: [
      {
        id: "openai",
        name: "OpenAI",
        description: "OpenAI models",
        recommended: true,
        supported: true,
        auth_mode: "api_key_required",
        protocol: "openai_chat_completions",
        default_base_url: "https://api.openai.com/v1",
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
      has_api_key: true,
      has_credential: false,
      credential_kind: "api_key",
      credential_source: "environment",
      auth_required: true
    },
    connected_provider: {
      id: "openai",
      name: "OpenAI",
      model: "gpt-5.5",
      base_url: "https://api.openai.com/v1"
    },
    last_connection: null,
    configured_providers: [
      {
        provider_id: "openai",
        base_url: "https://api.openai.com/v1",
        model: "gpt-5.5",
        has_api_key: true,
        has_credential: false,
        credential_kind: "api_key",
        credential_source: "environment",
        auth_required: true
      }
    ],
    model_options: [
      modelOption("gpt-4.1-mini"),
      modelOption("gpt-5.5")
    ]
  };
}

function modelOption(modelId: string): ProviderModelView {
  return {
    provider_id: "openai",
    id: modelId,
    display_name: modelId,
    context_window: 128000,
    supports_tools: true,
    capabilities: {
      supports_tools: true,
      thinking: {
        supported: false,
        modes: []
      },
      reasoning: null,
      input_modalities: ["text", "image"]
    }
  };
}

function runtimeSettings(): RuntimeSettingsResponse {
  return {
    default_model: "gpt-5.5",
    default_thinking_mode: null,
    presets: [],
    mcp_servers: [],
    skill_roots: []
  };
}

function createDeferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((promiseResolve, promiseReject) => {
    resolve = promiseResolve;
    reject = promiseReject;
  });
  return { promise, resolve, reject };
}
