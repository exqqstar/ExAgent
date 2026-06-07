import { Activity, Bot, Brain, CircleAlert, Gauge, MessageSquareText, Wrench } from "lucide-react";
import { useMemo, useState } from "react";
import { TokenUsagePanel, tokenUsageSummary } from "@/components/TokenUsagePanel";
import { TranscriptList } from "@/components/TranscriptList";
import { Badge } from "@/components/ui/badge";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import type { AgentNode, AgentRunStatus, AgentThreadView, RuntimeEvent, ThreadTokenUsage, TranscriptMessage } from "@/types";
import { cn } from "@/lib/utils";

type AgentThreadTab = "conversation" | "reasoning" | "tools" | "events";

const tabs: Array<{
  id: AgentThreadTab;
  label: string;
  icon: typeof MessageSquareText;
}> = [
  { id: "conversation", label: "Conversation", icon: MessageSquareText },
  { id: "reasoning", label: "Reasoning", icon: Brain },
  { id: "tools", label: "Tools", icon: Wrench },
  { id: "events", label: "Events", icon: Activity }
];

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

const eventVariant = {
  neutral: "neutral",
  info: "info",
  warning: "warning",
  danger: "danger",
  success: "success"
} as const;

export function AgentThreadViewer({
  agent,
  view,
  tokenUsage,
  onClose
}: {
  agent: AgentNode | null;
  view: AgentThreadView | null;
  tokenUsage?: ThreadTokenUsage | null;
  onClose: () => void;
}) {
  const [activeTab, setActiveTab] = useState<AgentThreadTab>("conversation");
  const threadId = view?.threadId ?? agent?.threadId ?? null;
  const open = Boolean(threadId);
  const groups = useMemo(() => groupTranscript(view?.transcript ?? []), [view?.transcript]);

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => !nextOpen && onClose()}>
      <DialogContent className="flex h-[min(720px,calc(100dvh-48px))] w-[min(880px,calc(100vw-32px))] max-w-none flex-col gap-0 overflow-hidden p-0">
        <DialogHeader className="shrink-0 border-b border-border px-5 py-4 pr-12">
          <div className="grid min-w-0 grid-cols-[1.25rem_minmax(0,1fr)] items-start gap-x-3">
            <Bot className="mt-1 h-4 w-4 shrink-0 text-subtle" />
            <div className="min-w-0">
              <div className="flex min-w-0 items-center gap-2">
                <DialogTitle className="min-w-0 truncate">
                  {agent?.name ?? compactThreadId(threadId ?? "")}
                </DialogTitle>
                {agent?.agentType && agent.agentType !== "worker" ? (
                  <Badge variant="neutral">{agent.agentType}</Badge>
                ) : null}
                {agent ? (
                  <Badge variant={statusBadgeVariant[agent.status]}>{statusLabel[agent.status]}</Badge>
                ) : null}
              </div>
              <DialogDescription className="mt-1.5 min-w-0 truncate">
                {agent?.task || agent?.agentPath || threadId || "Agent thread"}
              </DialogDescription>
              <div className="mt-2 grid min-w-0 gap-1">
                {threadId ? <MetaRow label="thread" value={compactThreadId(threadId)} title={threadId} mono /> : null}
                {agent?.agentPath ? <MetaRow label="path" value={agent.agentPath} mono /> : null}
              </div>
            </div>
          </div>
          {threadId ? (
            <div className="mt-3 border-t border-border pt-3">
              <div className="mb-1.5 flex min-w-0 items-center justify-between gap-3">
                <div className="flex min-w-0 items-center gap-2">
                  <Gauge className="h-3.5 w-3.5 shrink-0 text-subtle" />
                  <span className="type-label-md text-ink">Token Usage</span>
                </div>
                <span className="type-body-sm shrink-0 text-muted">{tokenUsageSummary(tokenUsage)}</span>
              </div>
              <TokenUsagePanel usage={tokenUsage} />
            </div>
          ) : null}
        </DialogHeader>

        <div className="flex shrink-0 items-center gap-1 border-b border-border px-3 py-2" role="tablist" aria-label="Agent thread views">
          {tabs.map((tab) => {
            const Icon = tab.icon;
            const selected = activeTab === tab.id;
            return (
              <button
                key={tab.id}
                type="button"
                role="tab"
                aria-selected={selected}
                className={cn(
                  "type-label-sm flex h-8 items-center gap-1.5 rounded-md px-2.5 text-muted transition-colors hover:bg-surface-2 hover:text-ink focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus",
                  selected && "bg-surface-2 text-ink"
                )}
                onClick={() => setActiveTab(tab.id)}
              >
                <Icon className="h-3.5 w-3.5" />
                {tab.label}
              </button>
            );
          })}
        </div>

        {view?.error ? (
          <div className="mx-5 mt-4 flex shrink-0 items-start gap-2 text-danger">
            <CircleAlert className="mt-0.5 h-4 w-4 shrink-0" />
            <p className="type-body-sm min-w-0 break-words">{view.error}</p>
          </div>
        ) : null}

        <ScrollArea className="min-h-0 flex-1">
          <div className="min-w-0 px-5 py-4">
            <AgentThreadTabPanel
              activeTab={activeTab}
              view={view}
              conversation={groups.conversation}
              reasoning={groups.reasoning}
              tools={groups.tools}
            />
          </div>
        </ScrollArea>
      </DialogContent>
    </Dialog>
  );
}

function AgentThreadTabPanel({
  activeTab,
  view,
  conversation,
  reasoning,
  tools
}: {
  activeTab: AgentThreadTab;
  view: AgentThreadView | null;
  conversation: TranscriptMessage[];
  reasoning: TranscriptMessage[];
  tools: TranscriptMessage[];
}) {
  if (activeTab === "conversation") {
    return (
      <TranscriptList
        messages={conversation}
        loading={view?.loading ?? false}
        emptyLabel="No conversation recorded for this agent."
        className="gap-4"
      />
    );
  }

  if (activeTab === "reasoning") {
    return (
      <TranscriptList
        messages={reasoning}
        loading={view?.loading ?? false}
        emptyLabel="No reasoning recorded for this agent."
        className="gap-4"
      />
    );
  }

  if (activeTab === "tools") {
    return (
      <TranscriptList
        messages={tools}
        loading={view?.loading ?? false}
        emptyLabel="No tool activity recorded for this agent."
        className="gap-4"
      />
    );
  }

  return <RecentAgentEvents events={view?.events ?? []} loading={view?.loading ?? false} />;
}

function RecentAgentEvents({ events, loading }: { events: RuntimeEvent[]; loading: boolean }) {
  if (loading) {
    return <TranscriptList messages={[]} loading />;
  }

  if (events.length === 0) {
    return <p className="type-body-md text-muted">No runtime events recorded for this agent.</p>;
  }

  return (
    <div className="space-y-2.5">
      {events.map((event) => (
        <div key={event.id} className="min-w-0 border-l border-border pl-3">
          <div className="flex min-w-0 items-center justify-between gap-2">
            <span className="type-label-md min-w-0 truncate text-ink">{event.label}</span>
            <Badge variant={eventVariant[event.tone ?? "neutral"]}>{event.timestamp}</Badge>
          </div>
          <p className="type-body-sm mt-0.5 break-words text-muted">{event.detail}</p>
        </div>
      ))}
    </div>
  );
}

function groupTranscript(messages: TranscriptMessage[]) {
  return {
    conversation: messages.filter((message) =>
      message.role === "user" || message.role === "assistant" || message.role === "system"
    ),
    reasoning: messages.filter((message) => message.role === "reasoning"),
    tools: messages.filter((message) => message.role === "tool" || message.role === "approval")
  };
}

function MetaRow({
  label,
  value,
  title,
  mono
}: {
  label: string;
  value: string;
  title?: string;
  mono?: boolean;
}) {
  return (
    <div className="grid min-w-0 grid-cols-[52px_minmax(0,1fr)] items-start gap-2">
      <dt className="type-label-sm text-muted">{label}</dt>
      <dd title={title ?? value} className={cn("type-body-sm min-w-0 truncate text-muted", mono && "type-code-sm")}>
        {value}
      </dd>
    </div>
  );
}

function compactThreadId(threadId: string) {
  const tail = threadId.split(/[_-]/).filter(Boolean).pop() ?? threadId;
  return tail.length > 12 ? `...${tail.slice(-12)}` : tail;
}
