import type { BackendRuntimeEvent, RuntimeEvent } from "@/types";

export type RuntimeEventKindType = BackendRuntimeEvent["kind"]["type"];

export type EventTurnGroup = {
  turnId: string | null;
  events: BackendRuntimeEvent[];
};

export function runtimeEventToInspector(event: BackendRuntimeEvent): RuntimeEvent {
  return {
    id: event.event_id,
    label: event.kind.type.replaceAll("_", " "),
    detail: eventDetail(event),
    timestamp: "now",
    tone: eventTone(event)
  };
}

export function runtimeEventsToInspector(events: BackendRuntimeEvent[]): RuntimeEvent[] {
  return events.filter(shouldShowInspectorEvent).map(runtimeEventToInspector);
}

export function shouldShowInspectorEvent(event: BackendRuntimeEvent) {
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

export function isDeltaEvent(kindType: RuntimeEventKindType): boolean {
  switch (kindType) {
    case "assistant_text_delta":
    case "reasoning_delta":
    case "tool_invocation_output_delta":
    case "exec_output":
      return true;
    default:
      return false;
  }
}

export function eventTurnId(event: BackendRuntimeEvent): string | null {
  return event.turn_id ?? null;
}

export function groupEventsByTurn(events: BackendRuntimeEvent[]): EventTurnGroup[] {
  const groups: EventTurnGroup[] = [];
  const groupByTurnId = new Map<string, EventTurnGroup>();
  let noTurnGroup: EventTurnGroup | null = null;

  for (const event of events) {
    const turnId = eventTurnId(event);
    let group: EventTurnGroup | undefined;
    if (turnId) {
      group = groupByTurnId.get(turnId);
      if (!group) {
        group = { turnId, events: [] };
        groupByTurnId.set(turnId, group);
        groups.push(group);
      }
    } else {
      if (!noTurnGroup) {
        noTurnGroup = { turnId: null, events: [] };
        groups.push(noTurnGroup);
      }
      group = noTurnGroup;
    }

    group.events.push(event);
  }

  return groups;
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
