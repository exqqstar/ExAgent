import "@testing-library/jest-dom/vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { AgentsPanel } from "@/components/AgentsPanel";
import type { AgentNode } from "@/types";

function agent(overrides: Partial<AgentNode>): AgentNode {
  return {
    threadId: "thread-agent",
    parentThreadId: "thread-root",
    name: "agent",
    agentPath: "root/agent",
    status: "running",
    task: "work on the task",
    lastActivity: null,
    isRoot: false,
    children: [],
    ...overrides
  };
}

function root(children: AgentNode[]): AgentNode {
  return {
    threadId: "thread-root",
    parentThreadId: null,
    name: "Root agent",
    agentPath: null,
    status: "running",
    task: "",
    lastActivity: null,
    isRoot: true,
    children
  };
}

describe("AgentsPanel", () => {
  it("renders current tool as the compact row activity with token count", () => {
    render(
      <AgentsPanel
        root={root([
          agent({
            threadId: "thread-research",
            name: "researcher",
            currentTool: "run_command cargo test --test app_server_boundary",
            tokensUsed: 12420
          })
        ])}
      />
    );

    const row = screen.getByRole("treeitem", {
      name: /researcher, running, tool run_command cargo test --test app_server_boundary, 12\.4k tokens/
    });
    const tool = within(row).getByText("run_command cargo test --test app_server_boundary");

    expect(tool).toHaveClass("type-code-sm");
    expect(tool).toHaveClass("truncate");
    expect(within(row).getByText("12.4k")).toHaveClass("text-muted");
    expect(within(row).queryByText("work on the task")).not.toBeInTheDocument();
  });

  it("pins waiting-approval siblings first and gives them warning treatment", () => {
    render(
      <AgentsPanel
        root={root([
          agent({ threadId: "thread-runner", name: "runner", status: "running" }),
          agent({
            threadId: "thread-reviewer",
            name: "reviewer",
            status: "waiting_approval",
            currentTool: "edit_file"
          }),
          agent({ threadId: "thread-finished", name: "finished", status: "done" })
        ])}
      />
    );

    const rows = screen.getAllByRole("treeitem");
    expect(rows.map((row) => row.getAttribute("aria-label"))).toEqual([
      "Root agent, running",
      "reviewer, needs approval, tool edit_file",
      "runner, running, activity work on the task",
      "finished, done, activity work on the task"
    ]);

    const waitingRow = rows[1];
    expect(within(waitingRow).getByText("needs approval")).toBeInTheDocument();
    expect(within(waitingRow).getByTestId("waiting-approval-icon")).toBeInTheDocument();
    expect(waitingRow.firstElementChild).toHaveClass("border-warning");
  });

  it("expands all waiting nodes from an external panel-header signal", async () => {
    const user = userEvent.setup();
    const { rerender } = render(
      <AgentsPanel
        expandWaitingSignal={0}
        root={root([
          agent({
            threadId: "thread-runner",
            name: "runner",
            status: "running",
            children: [
              agent({
                threadId: "thread-nested",
                parentThreadId: "thread-runner",
                name: "nested reviewer",
                status: "waiting_approval",
                currentTool: "request_approval"
              })
            ]
          })
        ])}
      />
    );

    expect(screen.queryByRole("button", { name: "Expand 1 waiting approval agent" })).not.toBeInTheDocument();

    screen.getByRole("treeitem", { name: /nested reviewer, needs approval/ });
    await user.click(screen.getByRole("button", { name: "Collapse runner" }));
    expect(screen.queryByRole("treeitem", { name: /nested reviewer/ })).not.toBeInTheDocument();

    rerender(
      <AgentsPanel
        expandWaitingSignal={1}
        root={root([
          agent({
            threadId: "thread-runner",
            name: "runner",
            status: "running",
            children: [
              agent({
                threadId: "thread-nested",
                parentThreadId: "thread-runner",
                name: "nested reviewer",
                status: "waiting_approval",
                currentTool: "request_approval"
              })
            ]
          })
        ])}
      />
    );

    expect(screen.getByRole("treeitem", { name: /nested reviewer, needs approval/ })).toBeInTheDocument();
  });

  it("auto-expands the path to a newly waiting-approval node", () => {
    const child = agent({
      threadId: "thread-runner",
      name: "runner",
      children: [
        agent({
          threadId: "thread-nested",
          parentThreadId: "thread-runner",
          name: "nested reviewer",
          status: "running"
        })
      ]
    });
    const { rerender } = render(<AgentsPanel root={root([child])} />);

    expect(screen.queryByRole("treeitem", { name: /nested reviewer/ })).not.toBeInTheDocument();

    rerender(
      <AgentsPanel
        root={root([
          {
            ...child,
            children: [
              {
                ...child.children[0],
                status: "waiting_approval",
                currentTool: "request_approval"
              }
            ]
          }
        ])}
      />
    );

    expect(screen.getByRole("treeitem", { name: /nested reviewer, needs approval/ })).toBeInTheDocument();
  });

  it("keeps keyboard selection through visible rows working", async () => {
    const user = userEvent.setup();
    const onSelectAgent = vi.fn();
    render(
      <AgentsPanel
        root={root([agent({ threadId: "thread-runner", name: "runner" })])}
        onSelectAgent={onSelectAgent}
      />
    );

    const rowButton = screen.getByRole("button", { name: /Open runner agent thread/ });
    rowButton.focus();
    await user.keyboard("{Enter}");

    expect(onSelectAgent).toHaveBeenCalledWith("thread-runner");
  });
});
