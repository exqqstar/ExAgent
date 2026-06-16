import { ChevronRight, CircleAlert, MessageSquareText } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Badge } from "@/components/ui/badge";
import type { AgentNode, AgentRunStatus } from "@/types";
import { useI18n, type TranslationKey } from "@/lib/i18n";
import { cn } from "@/lib/utils";

const statusBadgeVariant: Record<AgentRunStatus, "success" | "info" | "neutral" | "warning" | "danger"> = {
  running: "success",
  spawning: "info",
  waiting_approval: "warning",
  done: "neutral",
  idle: "neutral",
  failed: "danger"
};

const statusDotClass: Record<AgentRunStatus, string> = {
  running: "bg-success motion-safe:animate-pulse",
  spawning: "bg-info motion-safe:animate-pulse",
  waiting_approval: "bg-warning ring-2 ring-warning/25 motion-safe:animate-pulse",
  done: "bg-subtle",
  idle: "bg-muted",
  failed: "bg-danger"
};

export function AgentsPanel({
  root,
  selectedThreadId,
  onSelectAgent,
  expandWaitingSignal = 0
}: {
  root: AgentNode;
  selectedThreadId?: string | null;
  onSelectAgent?: (threadId: string) => void;
  expandWaitingSignal?: number;
}) {
  const { t } = useI18n();

  return (
    <ul role="tree" aria-label={t("agents.treeLabel")} className="min-w-0 space-y-1 overflow-hidden">
      <AgentTreeItem
        node={root}
        level={1}
        selectedThreadId={selectedThreadId}
        onSelectAgent={onSelectAgent}
        expandWaitingSignal={expandWaitingSignal}
      />
    </ul>
  );
}

function AgentTreeItem({
  node,
  level,
  selectedThreadId,
  onSelectAgent,
  expandWaitingSignal
}: {
  node: AgentNode;
  level: number;
  selectedThreadId?: string | null;
  onSelectAgent?: (threadId: string) => void;
  expandWaitingSignal: number;
}) {
  const { t } = useI18n();
  const sortedChildren = useMemo(() => sortChildrenForPanel(node.children), [node.children]);
  const descendantWaitingIds = useMemo(() => collectWaitingApprovalIds(sortedChildren), [sortedChildren]);
  const descendantWaitingKey = descendantWaitingIds.join("|");
  const hasChildren = sortedChildren.length > 0;
  const [open, setOpen] = useState(() => hasChildren && descendantWaitingIds.length > 0);
  const previousWaitingIds = useRef(new Set(descendantWaitingIds));
  const activity = getNodeActivity(node);
  const tokenCount = formatCompactTokenCount(node.tokensUsed);
  const ariaLabel = agentAriaLabel(node, activity, tokenCount, t);

  useEffect(() => {
    if (!hasChildren) {
      previousWaitingIds.current = new Set();
      return;
    }

    const previous = previousWaitingIds.current;
    if (descendantWaitingIds.some((threadId) => !previous.has(threadId))) {
      setOpen(true);
    }
    previousWaitingIds.current = new Set(descendantWaitingIds);
  }, [descendantWaitingIds, descendantWaitingKey, hasChildren]);

  useEffect(() => {
    if (expandWaitingSignal > 0 && descendantWaitingIds.length > 0) {
      setOpen(true);
    }
  }, [descendantWaitingIds.length, descendantWaitingKey, expandWaitingSignal]);

  if (node.isRoot) {
    return (
      <li
        role="treeitem"
        aria-level={level}
        aria-expanded={hasChildren ? true : undefined}
        aria-label={ariaLabel}
        className="min-w-0"
      >
        <div className="flex min-w-0 items-center gap-2 px-1.5 py-1">
          <StatusDot status={node.status} />
          <span className="type-label-md min-w-0 flex-1 truncate text-ink">{node.name}</span>
          <StatusBadge status={node.status} />
        </div>
        {hasChildren ? (
          <ChildGroup>
            {renderChildren(sortedChildren, level, selectedThreadId, onSelectAgent, expandWaitingSignal)}
          </ChildGroup>
        ) : null}
      </li>
    );
  }

  const selected = selectedThreadId === node.threadId;
  const needsApproval = node.status === "waiting_approval";
  return (
    <li
      role="treeitem"
      aria-level={level}
      aria-expanded={hasChildren ? open : undefined}
      aria-selected={selected}
      aria-label={ariaLabel}
      className="min-w-0"
    >
      <div
        className={cn(
          "flex w-full min-w-0 items-start gap-1 overflow-hidden rounded-md border border-transparent",
          needsApproval && "border-warning bg-warning/8",
          selected && "bg-surface-2 ring-1 ring-border"
        )}
      >
        {hasChildren ? (
          <button
            type="button"
            aria-label={(open ? t("agents.collapse") : t("agents.expand")).replace("{name}", node.name)}
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
          aria-label={openAgentLabel(node, activity, tokenCount, t)}
          onClick={() => {
            setOpen(true);
            onSelectAgent?.(node.threadId);
          }}
          className={cn(
            "flex w-0 min-w-0 flex-1 items-start gap-1.5 rounded-md px-1.5 py-1 text-left transition-colors hover:bg-surface-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-focus"
          )}
        >
          <StatusDot status={node.status} className="mt-[5px]" />
          <span className="min-w-0 flex-1 overflow-hidden">
            <span className="flex min-w-0 items-center gap-2">
              <span className="type-label-md min-w-0 flex-1 truncate text-ink">{node.name}</span>
              {node.agentType && node.agentType !== "worker" ? (
                <Badge variant="neutral">{node.agentType}</Badge>
              ) : null}
              <StatusBadge status={node.status} />
            </span>
            {activity ? (
              <span
                title={activity.text}
                className={cn(
                  "mt-0.5 block min-w-0 truncate",
                  activity.kind === "tool" ? "type-code-sm text-ink/80" : "type-body-sm text-muted"
                )}
              >
                {activity.text}
              </span>
            ) : null}
          </span>
          {tokenCount ? (
            <span
              title={`${node.tokensUsed?.toLocaleString() ?? tokenCount} ${t("agents.tokens")}`}
              className="type-code-sm mt-0.5 w-[3.5rem] shrink-0 text-right text-muted"
            >
              {tokenCount}
            </span>
          ) : null}
        </button>
        <button
          type="button"
          aria-label={t("agents.inspect").replace("{name}", node.name)}
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
              {renderChildren(sortedChildren, level, selectedThreadId, onSelectAgent, expandWaitingSignal)}
            </ul>
          ) : null}
        </div>
      ) : null}
    </li>
  );
}

function renderChildren(
  children: AgentNode[],
  level: number,
  selectedThreadId?: string | null,
  onSelectAgent?: (threadId: string) => void,
  expandWaitingSignal = 0
) {
  return children.map((child) => (
    <AgentTreeItem
      key={child.threadId}
      node={child}
      level={level + 1}
      selectedThreadId={selectedThreadId}
      onSelectAgent={onSelectAgent}
      expandWaitingSignal={expandWaitingSignal}
    />
  ));
}

function ChildGroup({ children }: { children: ReactNode }) {
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
  const { t } = useI18n();
  const needsApproval = status === "waiting_approval";
  return (
    <Badge variant={statusBadgeVariant[status]} className={needsApproval ? "gap-1" : undefined}>
      {needsApproval ? (
        <CircleAlert aria-hidden data-testid="waiting-approval-icon" className="h-3 w-3 shrink-0" />
      ) : null}
      <span>{agentStatusLabel(status, t)}</span>
    </Badge>
  );
}

type NodeActivity = {
  kind: "tool" | "activity";
  text: string;
};

function getNodeActivity(node: AgentNode): NodeActivity | null {
  const currentTool = normalizedText(node.currentTool);
  if (currentTool) {
    return { kind: "tool", text: currentTool };
  }

  const lastActivity = normalizedText(node.lastActivity);
  if (lastActivity) {
    return { kind: "activity", text: lastActivity };
  }

  const task = normalizedText(node.task);
  if (task) {
    return { kind: "activity", text: task };
  }

  return null;
}

function normalizedText(value: string | null | undefined): string | null {
  const text = value?.trim();
  return text ? text : null;
}

function agentAriaLabel(
  node: AgentNode,
  activity: NodeActivity | null,
  tokenCount: string | null,
  t: (key: TranslationKey) => string
) {
  const parts = [`${node.name}, ${agentStatusLabel(node.status, t)}`];
  if (activity) {
    parts.push(`${activity.kind === "tool" ? t("agents.tool") : t("agents.activity")} ${activity.text}`);
  }
  if (tokenCount) {
    parts.push(`${tokenCount} ${t("agents.tokens")}`);
  }
  return parts.join(", ");
}

function openAgentLabel(
  node: AgentNode,
  activity: NodeActivity | null,
  tokenCount: string | null,
  t: (key: TranslationKey) => string
) {
  const details = agentAriaLabel(node, activity, tokenCount, t);
  return t("agents.openThread").replace("{name}", node.name).replace("{details}", details);
}

function formatCompactTokenCount(tokensUsed: number | null | undefined): string | null {
  if (tokensUsed == null) {
    return null;
  }

  return new Intl.NumberFormat("en", {
    notation: "compact",
    maximumFractionDigits: 1
  })
    .format(tokensUsed)
    .toLowerCase();
}

function sortChildrenForPanel(children: AgentNode[]): AgentNode[] {
  return children
    .map((child, index) => ({ child, index }))
    .sort((left, right) => {
      const leftWaiting = left.child.status === "waiting_approval" ? 1 : 0;
      const rightWaiting = right.child.status === "waiting_approval" ? 1 : 0;
      return rightWaiting - leftWaiting || left.index - right.index;
    })
    .map(({ child }) => child);
}

function collectWaitingApprovalIds(nodes: AgentNode[]): string[] {
  const ids: string[] = [];
  const visit = (node: AgentNode) => {
    if (node.status === "waiting_approval") {
      ids.push(node.threadId);
    }
    node.children.forEach(visit);
  };
  nodes.forEach(visit);
  return ids.sort();
}

function agentStatusLabel(status: AgentRunStatus, t: (key: TranslationKey) => string) {
  switch (status) {
    case "running":
      return t("status.agent.running");
    case "spawning":
      return t("status.agent.spawning");
    case "waiting_approval":
      return t("status.agent.waitingApproval");
    case "done":
      return t("status.agent.done");
    case "failed":
      return t("status.agent.failed");
    case "idle":
    default:
      return t("status.agent.idle");
  }
}
