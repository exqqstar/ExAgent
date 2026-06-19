import "@testing-library/jest-dom/vitest";
import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { EventDetail } from "@/components/events/EventDetail";
import type { BackendRuntimeEvent, BackendRuntimeEventKind, TokenUsageInfo } from "@/types";

function event(eventId: string, kind: BackendRuntimeEventKind): BackendRuntimeEvent {
  return {
    event_id: eventId,
    thread_id: "thread-root",
    turn_id: "turn-1",
    kind
  };
}

const tokenInfo: TokenUsageInfo = {
  total_token_usage: {
    input_tokens: 1000,
    cached_input_tokens: 100,
    output_tokens: 250,
    reasoning_output_tokens: 50,
    total_tokens: 1300
  },
  last_token_usage: {
    input_tokens: 700,
    cached_input_tokens: 50,
    output_tokens: 125,
    reasoning_output_tokens: 25,
    total_tokens: 850
  },
  model_context_window: 200000
};

describe("EventDetail", () => {
  it("renders token count details through the token usage panel", () => {
    render(<EventDetail event={event("evt_token", { type: "token_count", info: tokenInfo })} allEvents={[]} />);

    expect(screen.getByText("evt_token")).toBeInTheDocument();
    expect(screen.getByText("turn-1")).toBeInTheDocument();
    expect(screen.getByText("thread total")).toBeInTheDocument();
    expect(screen.getByText("1,300")).toBeInTheDocument();
    expect(screen.getByText("last turn")).toBeInTheDocument();
    expect(screen.getByText("850")).toBeInTheDocument();
  });

  it("renders tool result status, output, and metadata", () => {
    render(
      <EventDetail
        event={event("evt_tool", {
          type: "tool_result",
          result: {
            tool_call_id: "call-1",
            tool_name: "read_file",
            content: "file contents",
            status: "success",
            meta: { bytes: 12 }
          }
        })}
        allEvents={[]}
      />
    );

    expect(screen.getByText("read_file")).toBeInTheDocument();
    expect(screen.getByText("success")).toBeInTheDocument();
    expect(screen.getByText("file contents")).toBeInTheDocument();
    expect(screen.getByText(/"bytes": 12/)).toBeInTheDocument();
    expect(screen.getByTestId("event-detail")).toHaveClass("h-full", "w-full", "overflow-hidden");
    expect(screen.getByTestId("event-detail-scroll")).toHaveClass("flex-1", "overflow-hidden");
    expect(screen.getByTestId("event-detail-header")).toHaveClass(
      "grid",
      "w-full",
      "grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto]"
    );
    expect(screen.getByTestId("event-detail-header")).not.toHaveClass("rounded-md", "border", "bg-surface-2");
    expect(screen.getByTestId("event-detail-meta")).toHaveClass("contents");
    expect(within(screen.getByTestId("event-detail-meta")).queryByText("time")).not.toBeInTheDocument();
    expect(screen.getByTestId("event-detail-time")).toHaveClass("justify-self-end");
    expect(screen.getByTestId("event-detail-time")).toHaveTextContent("not recorded");

    const outputSection = screen.getByText("Output").closest("details");
    expect(outputSection).not.toHaveClass("rounded-md", "border", "bg-surface-2");

    const outputBlock = screen.getByText("file contents").closest("pre");
    expect(outputBlock).toHaveClass("border-l");
    expect(outputBlock).not.toHaveClass("overflow-auto");
    expect(outputBlock).not.toHaveClass("max-h-[360px]");
    expect(outputBlock).not.toHaveClass("border-t", "bg-surface-2", "p-3");
  });

  it("falls back to formatted JSON for unhandled event kinds", () => {
    const futureKind = { type: "future_event", payload: { value: "kept" } } as unknown as BackendRuntimeEventKind;

    render(<EventDetail event={event("evt_future", futureKind)} allEvents={[]} />);

    expect(screen.getByText(/"type": "future_event"/)).toBeInTheDocument();
    expect(screen.getByText(/"value": "kept"/)).toBeInTheDocument();
  });
});
