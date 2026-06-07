import { ChevronRight, MessageSquareText } from "lucide-react";
import { useState } from "react";
import { Badge } from "@/components/ui/badge";
import type { AgentNode, AgentRunStatus } from "@/types";
import { cn } from "@/lib/utils";

const statusLabel: Record<AgentRunStatus, string> = {
  running: "running",
  spawning: "spawning",
  done: "done",
  idle: "idle",
  failed: "failed"
};

const statusBadgeVariant: Record<AgentRunStatus, "success" | "info" | "neutral" | "danger"> = {
  running: "success",
  spawning: "info",
  done: "neutral",
  idle: "neutral",
  failed: "danger"
};

const statusDotClass: Record<AgentRunStatus, string> = {
  running: "bg-success motion-safe:animate-pulse",
  spawning: "bg-info motion-safe:animate-pulse",
  done: "bg-subtle",
  idle: "bg-muted",
  failed: "bg-danger"
};

export function AgentsPanel({
  root,
  selectedThreadId,
  onSelectAgent
}: {
  root: AgentNode;
  selectedThreadId?: string | null;
  onSelectAgent?: (threadId: string) => void;
}) {
  return (
    <ul role="tree" aria-label="Running agents" className="min-w-0 space-y-1 overflow-hidden">
      <AgentTreeItem
        node={root}
        level={1}
        selectedThreadId={selectedThreadId}
        onSelectAgent={onSelectAgent}
      />
    </ul>
  );
}

function AgentTreeItem({
  node,
  level,
  selectedThreadId,
  onSelectAgent
}: {
  node: AgentNode;
  level: number;
  selectedThreadId?: string | null;
  onSelectAgent?: (threadId: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const hasChildren = node.children.length > 0;

  if (node.isRoot) {
    return (
      <li
        role="treeitem"
        aria-level={level}
        aria-expanded={hasChildren ? true : undefined}
        aria-label={`${node.name}, ${statusLabel[node.status]}`}
        className="min-w-0"
      >
        <div className="flex items-center gap-2 px-1.5 py-1">
          <StatusDot status={node.status} />
          <span className="type-label-md min-w-0 flex-1 truncate text-ink">{node.name}</span>
          <StatusBadge status={node.status} />
        </div>
        {hasChildren ? (
          <ChildGroup>{renderChildren(node, level, selectedThreadId, onSelectAgent)}</ChildGroup>
        ) : null}
      </li>
    );
  }

  const selected = selectedThreadId === node.threadId;
  return (
    <li
      role="treeitem"
      aria-level={level}
      aria-expanded={hasChildren ? open : undefined}
      aria-selected={selected}
      aria-label={`${node.name}, ${statusLabel[node.status]}`}
      className="min-w-0"
    >
      <div
        className={cn(
          "flex w-full min-w-0 items-start gap-1 overflow-hidden rounded-md",
          selected && "bg-surface-2 ring-1 ring-border"
        )}
      >
        {hasChildren ? (
          <button
            type="button"
            aria-label={`${open ? "Collapse" : "Expand"} ${node.name}`}
            onClick={() => setOpen((value) => !value)}
            className="mt-0.5 flex h-6 w-5 shrink-0 items-center justify-center rounded text-subtle transition-colors hover:bg-surface-2 hover:text-ink focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-focus"
          >
            <ChevronRight
              aria-hidden
              className={cn("h-3.5 w-3.5 transition-transform", open && "rotate-90")}
            />
          </button>
        ) : (
          <span aria-hidden className="mt-0.5 h-6 w-5 shrink-0" />
        )}
        <button
          type="button"
          aria-label={`Open ${node.name} agent thread`}
          onClick={() => {
            setOpen(true);
            onSelectAgent?.(node.threadId);
          }}
          className={cn(
            "flex w-0 min-w-0 flex-1 items-start gap-1.5 rounded-md px-1.5 py-1 text-left transition-colors hover:bg-surface-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-focus"
          )}
        >
          <StatusDot status={node.status} className="mt-[5px]" />
          <span className="min-w-0 flex-1">
            <span className="flex items-center gap-2">
              <span className="type-label-md min-w-0 flex-1 truncate text-ink">{node.name}</span>
              {node.agentType && node.agentType !== "worker" ? (
                <Badge variant="neutral">{node.agentType}</Badge>
              ) : null}
              <StatusBadge status={node.status} />
            </span>
            {node.task ? (
              <span className="type-body-sm mt-0.5 block truncate text-muted">{node.task}</span>
            ) : null}
          </span>
        </button>
        <button
          type="button"
          aria-label={`Inspect ${node.name}`}
          onClick={() => onSelectAgent?.(node.threadId)}
          className="mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-subtle transition-colors hover:bg-surface-2 hover:text-ink focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-focus"
        >
          <MessageSquareText className="h-3.5 w-3.5" />
        </button>
      </div>
      {open ? (
        <div className="agent-reveal ml-[7px] border-l border-border pl-2.5">
          {hasChildren ? (
            <ul role="group" className="space-y-1 py-1">
              {node.children.map((child) => (
                <AgentTreeItem
                  key={child.threadId}
                  node={child}
                  level={level + 1}
                  selectedThreadId={selectedThreadId}
                  onSelectAgent={onSelectAgent}
                />
              ))}
            </ul>
          ) : null}
        </div>
      ) : null}
    </li>
  );
}

function renderChildren(
  node: AgentNode,
  level: number,
  selectedThreadId?: string | null,
  onSelectAgent?: (threadId: string) => void
) {
  return node.children.map((child) => (
    <AgentTreeItem
      key={child.threadId}
      node={child}
      level={level + 1}
      selectedThreadId={selectedThreadId}
      onSelectAgent={onSelectAgent}
    />
  ));
}

function ChildGroup({ children }: { children: React.ReactNode }) {
  return (
    <ul role="group" className="ml-[7px] min-w-0 space-y-1 overflow-hidden border-l border-border pl-2.5">
      {children}
    </ul>
  );
}

function StatusDot({ status, className }: { status: AgentRunStatus; className?: string }) {
  return (
    <span aria-hidden className={cn("h-1.5 w-1.5 shrink-0 rounded-full", statusDotClass[status], className)} />
  );
}

function StatusBadge({ status }: { status: AgentRunStatus }) {
  return <Badge variant={statusBadgeVariant[status]}>{statusLabel[status]}</Badge>;
}
