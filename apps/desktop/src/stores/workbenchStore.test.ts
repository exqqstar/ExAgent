import { beforeEach, describe, expect, it, vi } from "vitest";
import { exagentClient } from "@/api/exagentClient";
import { useWorkbenchStore } from "@/stores/workbenchStore";
import type { BackendRuntimeEvent } from "@/types";

describe("workbenchStore runtime events", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    useWorkbenchStore.setState(useWorkbenchStore.getInitialState(), true);
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

  it("keeps replayed token usage when a buffered live event has the same event id", async () => {
    const staleBufferedEvent = tokenCountEvent("evt-token-duplicate", 100);
    const freshReplayEvent = tokenCountEvent("evt-token-duplicate", 200);

    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "thread-root",
        status: "running",
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

function tokenCountEvent(eventId: string, totalTokens: number): BackendRuntimeEvent {
  return {
    event_id: eventId,
    thread_id: "thread-root",
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
