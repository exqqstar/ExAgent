import "@testing-library/jest-dom/vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AgentThreadViewer } from "@/components/AgentThreadViewer";
import type { AgentNode, AgentThreadView, ThreadTokenUsage } from "@/types";

const htmlElementScrollIntoView = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "scrollIntoView");
const htmlDivElementScrollIntoView = Object.getOwnPropertyDescriptor(HTMLDivElement.prototype, "scrollIntoView");

const agent: AgentNode = {
  threadId: "thread-review",
  parentThreadId: "thread-root",
  name: "review agent",
  agentPath: "root/reviewer",
  status: "running",
  task: "review the plan",
  lastActivity: null,
  agentType: "reviewer",
  role: null,
  nickname: null,
  isRoot: false,
  children: []
};

const view: AgentThreadView = {
  threadId: "thread-review",
  loading: false,
  error: null,
  transcript: [
    {
      id: "assistant-child",
      role: "assistant",
      body: "Child answer",
      timestamp: "history",
      threadId: "thread-review",
      turnId: "turn-1"
    },
    {
      id: "reasoning-child",
      role: "reasoning",
      title: "Reasoning",
      body: "Reasoned through the request",
      timestamp: "history",
      threadId: "thread-review",
      turnId: "turn-1"
    },
    {
      id: "tool-child",
      role: "tool",
      title: "read_file",
      body: "Opened src/main.rs",
      timestamp: "history",
      status: "success",
      threadId: "thread-review",
      turnId: "turn-1",
      toolStatus: "completed"
    }
  ],
  events: [
    {
      id: "event-tool",
      label: "tool invocation completed",
      detail: "read_file: success",
      timestamp: "now",
      tone: "success"
    }
  ]
};

const tokenUsage: ThreadTokenUsage = {
  threadId: "thread-review",
  total: {
    input_tokens: 1200,
    cached_input_tokens: 200,
    output_tokens: 300,
    reasoning_output_tokens: 100,
    total_tokens: 1600
  },
  last: {
    input_tokens: 800,
    cached_input_tokens: 100,
    output_tokens: 200,
    reasoning_output_tokens: 50,
    total_tokens: 1050
  },
  modelContextWindow: null
};

describe("AgentThreadViewer", () => {
  afterEach(() => {
    restorePropertyDescriptor(HTMLElement.prototype, "scrollIntoView", htmlElementScrollIntoView);
    restorePropertyDescriptor(HTMLDivElement.prototype, "scrollIntoView", htmlDivElementScrollIntoView);
  });

  it("opens as a tabbed dialog and keeps tool output off the default conversation tab", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();

    render(<AgentThreadViewer agent={agent} view={view} onClose={onClose} />);

    expect(screen.getByRole("dialog", { name: "review agent" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Conversation" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("Child answer")).toBeInTheDocument();
    expect(screen.queryByText("read_file")).not.toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "Tools" }));
    expect(screen.getByText("read_file")).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "Reasoning" }));
    expect(screen.getByLabelText("Reasoning message")).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "Events" }));
    expect(screen.getByText("tool invocation completed")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("keeps token usage collapsed by default and expands details on request", async () => {
    const user = userEvent.setup();

    render(<AgentThreadViewer agent={agent} view={view} tokenUsage={tokenUsage} onClose={vi.fn()} />);

    const tokenUsageToggle = screen.getByRole("button", { name: /Token Usage\s+1\.6k tokens/i });

    expect(tokenUsageToggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByText("1.6k tokens")).toBeInTheDocument();
    expect(screen.queryByText("thread total")).not.toBeInTheDocument();
    expect(screen.queryByText("1,600")).not.toBeInTheDocument();

    await user.click(tokenUsageToggle);

    expect(tokenUsageToggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("thread total")).toBeInTheDocument();
    expect(screen.getByText("1,600")).toBeInTheDocument();
    expect(screen.getByText("input")).toBeInTheDocument();
    expect(screen.getByText("1,200")).toBeInTheDocument();
    expect(screen.getByText("last turn")).toBeInTheDocument();
    expect(screen.getByText("1,050")).toBeInTheDocument();

    await user.click(tokenUsageToggle);

    expect(tokenUsageToggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("thread total")).not.toBeInTheDocument();
  });

  it("resets token usage details when switching child threads", async () => {
    const user = userEvent.setup();
    const secondAgent: AgentNode = {
      ...agent,
      threadId: "thread-second",
      name: "second agent",
      agentPath: "root/second"
    };
    const secondView: AgentThreadView = {
      ...view,
      threadId: "thread-second",
      transcript: []
    };
    const secondUsage: ThreadTokenUsage = {
      ...tokenUsage,
      threadId: "thread-second",
      total: {
        input_tokens: 2000,
        cached_input_tokens: 0,
        output_tokens: 200,
        reasoning_output_tokens: 0,
        total_tokens: 2200
      },
      last: {
        input_tokens: 1500,
        cached_input_tokens: 0,
        output_tokens: 100,
        reasoning_output_tokens: 0,
        total_tokens: 1600
      }
    };

    const { rerender } = render(
      <AgentThreadViewer agent={agent} view={view} tokenUsage={tokenUsage} onClose={vi.fn()} />
    );

    await user.click(screen.getByRole("button", { name: /Token Usage\s+1\.6k tokens/i }));
    expect(screen.getByText("thread total")).toBeInTheDocument();

    rerender(<AgentThreadViewer agent={secondAgent} view={secondView} tokenUsage={secondUsage} onClose={vi.fn()} />);

    const secondToggle = screen.getByRole("button", { name: /Token Usage\s+2\.2k tokens/i });
    expect(secondToggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("thread total")).not.toBeInTheDocument();
  });

  it("follows running transcript updates until the user pauses and jumps to latest", async () => {
    const user = userEvent.setup();
    const scrollIntoView = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      configurable: true,
      value: scrollIntoView
    });
    Object.defineProperty(HTMLDivElement.prototype, "scrollIntoView", {
      configurable: true,
      value: scrollIntoView
    });
    const { rerender } = render(<AgentThreadViewer agent={agent} view={view} onClose={vi.fn()} />);

    await waitFor(() => expect(scrollIntoView).toHaveBeenCalled());
    scrollIntoView.mockClear();

    const liveView = withAssistantMessage(view, "assistant-live-1", "Live child answer");
    rerender(<AgentThreadViewer agent={agent} view={liveView} onClose={vi.fn()} />);

    expect(screen.getByText("Live child answer")).toBeInTheDocument();
    await waitFor(() => expect(scrollIntoView).toHaveBeenCalled());
    expect(screen.queryByRole("button", { name: "Jump to latest" })).not.toBeInTheDocument();
    scrollIntoView.mockClear();

    const grownView = withMessageBody(liveView, "assistant-live-1", "Live child answer with streamed tail");
    rerender(<AgentThreadViewer agent={agent} view={grownView} onClose={vi.fn()} />);

    expect(screen.getByText("Live child answer with streamed tail")).toBeInTheDocument();
    await waitFor(() => expect(scrollIntoView).toHaveBeenCalled());

    const viewport = getScrollViewport();
    setScrollMetrics(viewport, { scrollHeight: 1000, clientHeight: 300, scrollTop: 100 });
    fireEvent.scroll(viewport);

    expect(screen.getByRole("button", { name: "Jump to latest" })).toBeInTheDocument();
    scrollIntoView.mockClear();

    const pausedView = withMessageBody(
      grownView,
      "assistant-live-1",
      "Live child answer with streamed tail while paused"
    );
    rerender(<AgentThreadViewer agent={agent} view={pausedView} onClose={vi.fn()} />);

    expect(screen.getByText("Live child answer with streamed tail while paused")).toBeInTheDocument();
    expect(scrollIntoView).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Jump to latest" }));

    await waitFor(() => expect(scrollIntoView).toHaveBeenCalled());
    expect(screen.queryByRole("button", { name: "Jump to latest" })).not.toBeInTheDocument();
  });
});

function withAssistantMessage(source: AgentThreadView, id: string, body: string): AgentThreadView {
  return {
    ...source,
    transcript: [
      ...source.transcript,
      {
        id,
        role: "assistant",
        body,
        timestamp: "now",
        threadId: source.threadId,
        turnId: id
      }
    ]
  };
}

function withMessageBody(source: AgentThreadView, id: string, body: string): AgentThreadView {
  return {
    ...source,
    transcript: source.transcript.map((message) => (message.id === id ? { ...message, body } : message))
  };
}

function getScrollViewport() {
  const viewport = document.querySelector("[data-radix-scroll-area-viewport]");
  if (!(viewport instanceof HTMLElement)) {
    throw new Error("Scroll viewport not found");
  }
  return viewport;
}

function setScrollMetrics(
  element: HTMLElement,
  metrics: { scrollHeight: number; clientHeight: number; scrollTop: number }
) {
  Object.defineProperties(element, {
    scrollHeight: { configurable: true, value: metrics.scrollHeight },
    clientHeight: { configurable: true, value: metrics.clientHeight },
    scrollTop: { configurable: true, value: metrics.scrollTop, writable: true }
  });
}

function restorePropertyDescriptor(target: object, key: "scrollIntoView", descriptor: PropertyDescriptor | undefined) {
  if (descriptor) {
    Object.defineProperty(target, key, descriptor);
    return;
  }
  delete (target as { scrollIntoView?: unknown }).scrollIntoView;
}
