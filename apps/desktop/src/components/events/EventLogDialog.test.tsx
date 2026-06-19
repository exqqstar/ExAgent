import "@testing-library/jest-dom/vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { EventLogDialog } from "@/components/events/EventLogDialog";
import * as exagentClient from "@/api/exagentClient";
import type { BackendRuntimeEvent, BackendRuntimeEventKind } from "@/types";

vi.mock("@/api/exagentClient", () => ({
  replayAllEvents: vi.fn(),
  exagentClient: {
    replayAllEvents: vi.fn()
  }
}));

class ResizeObserverMock {
  observe() {}
  unobserve() {}
  disconnect() {}
}

function event(eventId: string, kind: BackendRuntimeEventKind, turnId = "turn-1"): BackendRuntimeEvent {
  return {
    event_id: eventId,
    thread_id: "thread-root",
    turn_id: turnId,
    kind
  };
}

const events: BackendRuntimeEvent[] = [
  event("evt_assistant", {
    type: "assistant_turn",
    turn: {
      text: "Assistant answer",
      tool_calls: []
    }
  }),
  event("evt_tool", {
    type: "tool_result",
    result: {
      tool_call_id: "call-1",
      tool_name: "read_file",
      content: "tool output",
      status: "success"
    }
  }),
  event("evt_delta", { type: "assistant_text_delta", delta: "streamed" })
];

describe("EventLogDialog", () => {
  beforeEach(() => {
    vi.stubGlobal("ResizeObserver", ResizeObserverMock);
    vi.mocked(exagentClient.replayAllEvents).mockReset();
    vi.mocked(exagentClient.replayAllEvents).mockResolvedValue(events);
    vi.mocked(exagentClient.exagentClient.replayAllEvents).mockReset();
    vi.mocked(exagentClient.exagentClient.replayAllEvents).mockResolvedValue(events);
  });

  it("loads all events when opened and updates detail selection from the list", async () => {
    const user = userEvent.setup();

    render(<EventLogDialog projectId="project" threadId="thread-root" open onClose={vi.fn()} />);

    expect(screen.getByText("Loading events...")).toBeInTheDocument();
    expect(await screen.findByText("Event Log · 3")).toBeInTheDocument();
    expect(exagentClient.exagentClient.replayAllEvents).toHaveBeenCalledWith("project", "thread-root");
    expect(screen.getByRole("button", { name: /assistant turn/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /tool result/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /assistant text delta/ })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /tool result/ }));

    expect(screen.getByTestId("event-log-layout")).toHaveClass("h-full", "overflow-hidden");
    const detail = screen.getByTestId("event-log-detail");
    expect(detail).toHaveClass("flex", "min-w-0", "overflow-hidden");
    expect(detail).not.toHaveClass("rounded-md", "border", "bg-surface-1");
    expect(within(detail).getByText("evt_tool")).toBeInTheDocument();
    expect(within(detail).getByText("read_file")).toBeInTheDocument();
    expect(within(detail).getByText("tool output")).toBeInTheDocument();
  });

  it("reveals streaming delta events when the toggle is enabled", async () => {
    const user = userEvent.setup();

    render(<EventLogDialog projectId="project" threadId="thread-root" open onClose={vi.fn()} />);

    await screen.findByRole("button", { name: /assistant turn/ });
    expect(screen.queryByRole("button", { name: /assistant text delta/ })).not.toBeInTheDocument();

    await user.click(screen.getByRole("checkbox", { name: "Show streaming" }));

    expect(screen.getByRole("button", { name: /assistant text delta/ })).toBeInTheDocument();
  });
});
