import "@testing-library/jest-dom/vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { Inspector } from "@/components/Inspector";
import { useWorkbenchStore, type getWorkbenchState } from "@/stores/workbenchStore";
import type { AgentNode, ThreadTokenUsage } from "@/types";

type WorkbenchState = ReturnType<typeof getWorkbenchState>;

class ResizeObserverMock {
  observe() {}
  unobserve() {}
  disconnect() {}
}

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", ResizeObserverMock);
});

const rootOnly: AgentNode = {
  threadId: "thread-root",
  parentThreadId: null,
  name: "Root agent",
  agentPath: null,
  status: "idle",
  task: "",
  lastActivity: null,
  isRoot: true,
  children: []
};

const rootUsage: ThreadTokenUsage = {
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
};

const childUsage: ThreadTokenUsage = {
  threadId: "thread-child",
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

function childAgent(overrides: Partial<AgentNode>): AgentNode {
  return {
    threadId: "thread-child",
    parentThreadId: "thread-root",
    name: "researcher",
    agentPath: "root/researcher",
    status: "running",
    task: "inspect the panel",
    lastActivity: null,
    isRoot: false,
    children: [],
    ...overrides
  };
}

function workbenchState(agents: AgentNode[], overrides: Partial<WorkbenchState> = {}): WorkbenchState {
  return {
    ...useWorkbenchStore.getInitialState(),
    loading: false,
    error: null,
    agents,
    projects: [],
    sessions: [
      {
        id: "thread-root",
        projectId: "project",
        title: "Active thread",
        updatedAt: "now",
        status: "idle"
      }
    ],
    activeProjectId: "project",
    activeSessionId: "thread-root",
    transcript: [],
    events: [],
    changedFiles: [],
    cwd: "/tmp/project",
    policy: "local",
    tokenUsage: {
      input: 0,
      output: 0,
      limit: 1
    },
    runtimeSettings: null,
    selectedModel: null,
    selectedThinkingMode: null,
    openAgentThread: async () => undefined,
    closeAgentThread: () => undefined,
    ...overrides
  };
}

describe("Inspector Agents section", () => {
  it("uses the compact divided section structure", () => {
    const { container } = render(<Inspector state={workbenchState([rootOnly])} />);

    const section = container.querySelector(".inspector-section");
    expect(section).toBeInTheDocument();
    expect(section).toHaveClass("inspector-section");
    expect(section).not.toHaveClass("rounded-lg");
    expect(container.querySelector(".inspector-section-content")).toBeInTheDocument();
  });

  it("summarizes the total roster when no subagents are running", () => {
    const doneRoot: AgentNode = {
      ...rootOnly,
      children: [
        childAgent({ threadId: "thread-research", name: "researcher", status: "done" }),
        childAgent({ threadId: "thread-tests", name: "test-writer", status: "done" })
      ]
    };

    render(<Inspector state={workbenchState([doneRoot])} />);

    expect(screen.getByText("3 agents")).toBeInTheDocument();
  });

  it("opens the Agents section when running subagents appear after initial render", () => {
    const { rerender } = render(<Inspector state={workbenchState([rootOnly])} />);

    expect(screen.queryByRole("tree", { name: "Running agents" })).not.toBeInTheDocument();

    const runningRoot: AgentNode = {
      ...rootOnly,
      status: "running",
      children: [childAgent({ threadId: "thread-research", name: "researcher", status: "running" })]
    };
    rerender(<Inspector state={workbenchState([runningRoot])} />);

    expect(screen.getByRole("tree", { name: "Running agents" })).toBeInTheDocument();
  });

  it("renders projected agent type as read-only metadata", async () => {
    const runningRoot: AgentNode = {
      ...rootOnly,
      status: "running",
      children: [
        childAgent({
          threadId: "thread-review",
          name: "review agent",
          agentType: "reviewer",
          status: "running",
          task: "review the plan"
        })
      ]
    };

    render(<Inspector state={workbenchState([runningRoot])} />);

    const reviewerItem = screen.getByRole("treeitem", { name: "review agent, running" });
    expect(within(reviewerItem).getByText("reviewer")).toBeInTheDocument();

    expect(within(reviewerItem).queryByText("type")).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: /agent type/i })).not.toBeInTheDocument();
  });

  it("opens the selected agent thread viewer from an agent row", async () => {
    const user = userEvent.setup();
    const openAgentThread = vi.fn(async (_threadId: string) => undefined);
    const reviewAgent = childAgent({
      threadId: "thread-review",
      name: "review agent",
      agentType: "reviewer",
      status: "running",
      task: "review the plan"
    });
    const runningRoot: AgentNode = {
      ...rootOnly,
      status: "running",
      children: [reviewAgent]
    };

    const baseState = workbenchState([runningRoot], {
      openAgentThread
    });
    render(
      <Inspector
        state={baseState}
      />
    );

    await user.click(screen.getByRole("button", { name: "Inspect review agent" }));

    expect(openAgentThread).toHaveBeenCalledWith("thread-review");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});

describe("Inspector Token Usage section", () => {
  it("shows active root thread token totals without context percentage copy", () => {
    render(
      <Inspector
        state={workbenchState([rootOnly], {
          tokenUsageByThreadId: {
            "thread-root": rootUsage
          }
        })}
      />
    );

    expect(screen.getByRole("button", { name: /Token Usage/ })).toHaveTextContent("186.4k tokens");
    expect(screen.getByText("thread total")).toBeInTheDocument();
    expect(screen.getByText("186,400")).toBeInTheDocument();
    expect(screen.getByText("input")).toBeInTheDocument();
    expect(screen.getByText("142,000")).toBeInTheDocument();
    expect(screen.getByText("output")).toBeInTheDocument();
    expect(screen.getByText("31,200")).toBeInTheDocument();
    expect(screen.getByText("reasoning")).toBeInTheDocument();
    expect(screen.getByText("13,200")).toBeInTheDocument();
    expect(screen.getByText("cached input")).toBeInTheDocument();
    expect(screen.getByText("28,000")).toBeInTheDocument();
    expect(screen.getByText("last turn")).toBeInTheDocument();
    expect(screen.getByText("last input")).toBeInTheDocument();
    expect(screen.getByText("52,000")).toBeInTheDocument();
    // The Token Usage summary must not mix in context-window percentage copy.
    expect(screen.getByRole("button", { name: /Token Usage/ })).not.toHaveTextContent(/context/i);
    expect(screen.getByRole("button", { name: /Token Usage/ })).not.toHaveTextContent("%");
  });

  it("shows model context window usage in a dedicated section", () => {
    render(
      <Inspector
        state={workbenchState([rootOnly], {
          tokenUsageByThreadId: {
            "thread-root": rootUsage
          }
        })}
      />
    );

    expect(screen.getByRole("button", { name: /Context Window/ })).toHaveTextContent("15% used");
    expect(screen.getByText("window")).toBeInTheDocument();
    expect(screen.getByText("400,000")).toBeInTheDocument();
    expect(screen.getByText("in use")).toBeInTheDocument();
  });

  it("reports a missing context window without inventing a percentage", () => {
    render(
      <Inspector
        state={workbenchState([rootOnly], {
          tokenUsageByThreadId: {
            "thread-root": childUsage
          }
        })}
      />
    );

    expect(screen.getByRole("button", { name: /Context Window/ })).toHaveTextContent("not reported");
    expect(screen.getByText("No context window reported for this thread.")).toBeInTheDocument();
  });

  it("keeps token usage on the active root thread when a child agent is selected", () => {
    const selectedChild = childAgent({ threadId: "thread-child", name: "researcher", status: "running" });
    const runningRoot: AgentNode = {
      ...rootOnly,
      status: "running",
      children: [selectedChild]
    };

    render(
      <Inspector
        state={workbenchState([runningRoot], {
          selectedAgentThreadId: "thread-child",
          tokenUsageByThreadId: {
            "thread-root": rootUsage,
            "thread-child": childUsage
          }
        })}
      />
    );

    expect(screen.getByRole("button", { name: /Token Usage/ })).toHaveTextContent("186.4k tokens");
    expect(screen.getByText("186,400")).toBeInTheDocument();
    expect(screen.queryByText("1,600")).not.toBeInTheDocument();
  });

  it("uses an explicit not reported state when no token count event exists", () => {
    render(<Inspector state={workbenchState([rootOnly], { tokenUsageByThreadId: {} })} />);

    expect(screen.getByRole("button", { name: /Token Usage/ })).toHaveTextContent("not reported");
    expect(screen.getByText("No token usage reported for this thread.")).toBeInTheDocument();
  });
});
