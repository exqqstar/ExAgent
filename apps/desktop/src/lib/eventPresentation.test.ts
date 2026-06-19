import { describe, expect, it } from "vitest";
import {
  eventTurnId,
  groupEventsByTurn,
  isDeltaEvent,
  runtimeEventToInspector,
  shouldShowInspectorEvent
} from "@/lib/eventPresentation";
import type { BackendRuntimeEvent, BackendRuntimeEventKind } from "@/types";

function event(eventId: string, kind: BackendRuntimeEventKind, turnId?: string | null): BackendRuntimeEvent {
  return {
    event_id: eventId,
    thread_id: "thread-root",
    turn_id: turnId,
    kind
  };
}

describe("eventPresentation", () => {
  it("maps runtime events to inspector labels, details, and tones", () => {
    const inspectorEvent = runtimeEventToInspector(
      event("evt_assistant", {
        type: "assistant_turn",
        turn: {
          text: "Implemented the audit log.",
          tool_calls: []
        }
      })
    );

    expect(inspectorEvent).toEqual({
      id: "evt_assistant",
      label: "assistant turn",
      detail: "Implemented the audit log.",
      timestamp: "now",
      tone: "info"
    });

    expect(
      runtimeEventToInspector(
        event("evt_approval", {
          type: "approval_decision",
          approval_id: "approval-1",
          status: "approved",
          note: null
        })
      ).tone
    ).toBe("success");
  });

  it("keeps inspector filtering behavior compatible with the store", () => {
    expect(shouldShowInspectorEvent(event("evt_delta", { type: "assistant_text_delta", delta: "a" }))).toBe(false);
    expect(shouldShowInspectorEvent(event("evt_result", { type: "turn_completed" }))).toBe(true);
  });

  it("identifies streaming delta event kinds", () => {
    expect(isDeltaEvent("assistant_text_delta")).toBe(true);
    expect(isDeltaEvent("reasoning_delta")).toBe(true);
    expect(isDeltaEvent("tool_invocation_output_delta")).toBe(true);
    expect(isDeltaEvent("exec_output")).toBe(true);
    expect(isDeltaEvent("tool_invocation_completed")).toBe(false);
  });

  it("groups events by turn id and keeps thread-level events in the no-turn group", () => {
    const turnOneStart = event("evt_1", { type: "turn_started" }, "turn-1");
    const threadLevel = event("evt_thread", { type: "thread_goal_cleared", thread_id: "thread-root" }, null);
    const implicitThreadLevel = event("evt_thread_implicit", { type: "turn_interrupted" });
    const turnTwoStart = event("evt_2", { type: "turn_started" }, "turn-2");
    const turnOneEnd = event("evt_3", { type: "turn_completed" }, "turn-1");

    expect(eventTurnId(threadLevel)).toBeNull();
    expect(eventTurnId(implicitThreadLevel)).toBeNull();
    expect(groupEventsByTurn([turnOneStart, threadLevel, implicitThreadLevel, turnTwoStart, turnOneEnd])).toEqual([
      {
        turnId: "turn-1",
        events: [turnOneStart, turnOneEnd]
      },
      {
        turnId: null,
        events: [threadLevel, implicitThreadLevel]
      },
      {
        turnId: "turn-2",
        events: [turnTwoStart]
      }
    ]);
  });
});
