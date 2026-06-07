import "@testing-library/jest-dom/vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { AgentThreadViewer } from "@/components/AgentThreadViewer";
import type { AgentNode, AgentThreadView, ThreadTokenUsage } from "@/types";

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

  it("shows token usage for the selected child thread", () => {
    render(<AgentThreadViewer agent={agent} view={view} tokenUsage={tokenUsage} onClose={vi.fn()} />);

    expect(screen.getByText("Token Usage")).toBeInTheDocument();
    expect(screen.getByText("1.6k tokens")).toBeInTheDocument();
    expect(screen.getByText("thread total")).toBeInTheDocument();
    expect(screen.getByText("1,600")).toBeInTheDocument();
    expect(screen.getByText("input")).toBeInTheDocument();
    expect(screen.getByText("1,200")).toBeInTheDocument();
    expect(screen.getByText("last turn")).toBeInTheDocument();
    expect(screen.getByText("1,050")).toBeInTheDocument();
  });
});
