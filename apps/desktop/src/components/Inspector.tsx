import { Activity, Bot, ChevronRight, Cpu, Database, FileText, Gauge, HardDrive, ShieldCheck } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { AgentsPanel } from "@/components/AgentsPanel";
import { TokenUsagePanel, tokenUsageSummary } from "@/components/TokenUsagePanel";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import { countAgents, countLiveAgents } from "@/lib/agentTree";
import type { getWorkbenchState } from "@/stores/workbenchStore";
import { cn } from "@/lib/utils";

type WorkbenchState = ReturnType<typeof getWorkbenchState>;
type InspectorVariant = "sheet" | "panel";

const eventVariant = {
  neutral: "neutral",
  info: "info",
  warning: "warning",
  danger: "danger",
  success: "success"
} as const;

export function Inspector({ state, variant = "sheet" }: { state: WorkbenchState; variant?: InspectorVariant }) {
  const rootTokenUsage = state.activeSessionId ? state.tokenUsageByThreadId[state.activeSessionId] : null;
  const contextWindow = rootTokenUsage?.modelContextWindow ?? null;
  const contextUsed = rootTokenUsage?.last.total_tokens ?? null;
  const contextPercent =
    contextWindow && contextWindow > 0 && contextUsed != null
      ? Math.min(100, Math.round((contextUsed / contextWindow) * 100))
      : null;
  const activeSession = state.sessions.find((session) => session.id === state.activeSessionId);
  const activeStatus = activeSession?.status ?? "idle";
  const runtimeModel = state.selectedModel?.model_id ?? state.runtimeSettings?.default_model ?? "default";
  const thinkingMode = state.selectedThinkingMode ?? "default";
  const enabledMcpServers = state.runtimeSettings?.mcp_servers.filter((server) => server.enabled).length ?? 0;
  const enabledSkillRoots = state.runtimeSettings?.skill_roots.filter((root) => root.enabled).length ?? 0;
  const agentRoot = state.agents[0]?.threadId === state.activeSessionId ? state.agents[0] : null;
  const liveAgents = agentRoot ? countLiveAgents(state.agents) : 0;
  const totalAgents = agentRoot ? countAgents(state.agents) : 0;
  const agentSummary =
    liveAgents > 0 ? `${liveAgents} running` : `${totalAgents} ${totalAgents === 1 ? "agent" : "agents"}`;

  const content = (
    <div className="inspector-sections p-3">
      <InspectorSection
        defaultOpen
        icon={Activity}
        title="Progress"
        accessory={
          <Badge variant={activeStatus === "failed" ? "danger" : activeStatus === "awaiting_approval" ? "warning" : activeStatus === "running" ? "success" : "neutral"}>
            {activeStatus.replace("_", " ")}
          </Badge>
        }
      >
        <div className="mt-2 h-1.5 rounded-full bg-surface-3">
          <div className={cn("h-full rounded-full bg-primary", activeStatus === "running" ? "w-2/3" : "w-0")} />
        </div>
      </InspectorSection>

      {agentRoot ? (
        <InspectorSection
          defaultOpen={liveAgents > 0}
          icon={Bot}
          title="Agents"
          summary={agentSummary}
        >
          <AgentsPanel
            root={agentRoot}
            selectedThreadId={state.selectedAgentThreadId}
            onSelectAgent={state.openAgentThread}
          />
        </InspectorSection>
      ) : null}

      <InspectorSection icon={HardDrive} title="Environment" summary={compactPath(state.cwd)}>
        <KeyValue label="cwd" value={compactPath(state.cwd)} title={state.cwd} mono />
        <KeyValue label="policy" value={state.policy} />
      </InspectorSection>

      <InspectorSection defaultOpen icon={Cpu} title="Runtime" summary={runtimeModel}>
        <KeyValue label="model" value={runtimeModel} mono />
        <KeyValue label="thinking" value={thinkingMode} />
        <KeyValue label="MCP servers" value={`${enabledMcpServers} enabled`} />
        <KeyValue label="Skill roots" value={`${enabledSkillRoots} enabled`} />
      </InspectorSection>

      <InspectorSection defaultOpen icon={Gauge} title="Token Usage" summary={tokenUsageSummary(rootTokenUsage)}>
        <TokenUsagePanel usage={rootTokenUsage} />
      </InspectorSection>

      <InspectorSection
        defaultOpen
        icon={Database}
        title="Context Window"
        summary={contextPercent != null ? `${contextPercent}% used` : "not reported"}
      >
        {contextWindow && contextWindow > 0 ? (
          <div className="space-y-2">
            <KeyValue label="window" value={contextWindow.toLocaleString()} mono />
            <KeyValue label="in use" value={(contextUsed ?? 0).toLocaleString()} mono />
            <div className="mt-1 h-1.5 rounded-full bg-surface-3">
              <div className="h-full rounded-full bg-primary" style={{ width: `${contextPercent ?? 0}%` }} />
            </div>
          </div>
        ) : (
          <p className="type-body-md text-muted">No context window reported for this thread.</p>
        )}
      </InspectorSection>

      <InspectorSection
        icon={FileText}
        title="Changed Files"
        summary={state.changedFiles.length === 0 ? "none" : `${state.changedFiles.length} changed`}
      >
        <div className="space-y-2">
          {state.changedFiles.length === 0 ? (
            <p className="type-body-md text-muted">No changed files reported.</p>
          ) : (
            state.changedFiles.map((file) => (
              <div key={file.path} className="flex items-center gap-2 rounded-md bg-surface-2 px-2 py-1.5">
                <Badge variant={file.status === "deleted" ? "danger" : file.status === "added" ? "success" : "info"}>
                  {file.status}
                </Badge>
                <span className="type-code-sm min-w-0 truncate text-muted">{file.path}</span>
              </div>
            ))
          )}
        </div>
      </InspectorSection>

      <InspectorSection icon={ShieldCheck} title="Events" summary={`${state.events.length} recorded`}>
        <div className="space-y-2.5">
          {state.events.length === 0 ? (
            <p className="type-body-md text-muted">No runtime events yet.</p>
          ) : (
            state.events.map((event) => (
              <div key={event.id} className="min-w-0 border-l border-border pl-3">
                <div className="flex min-w-0 items-center justify-between gap-2">
                  <span className="type-label-md min-w-0 truncate text-ink">{event.label}</span>
                  <Badge variant={eventVariant[event.tone ?? "neutral"]}>{event.timestamp}</Badge>
                </div>
                <p className="type-body-sm mt-0.5 break-words text-muted">{event.detail}</p>
              </div>
            ))
          )}
        </div>
      </InspectorSection>
    </div>
  );

  if (variant === "panel") {
    return content;
  }

  return (
    <div className="flex h-full flex-col">
      <ScrollArea className="min-h-0 flex-1">
        {content}
      </ScrollArea>
    </div>
  );
}

function InspectorSection({
  icon: Icon,
  title,
  summary,
  accessory,
  defaultOpen = false,
  children
}: {
  icon: typeof Activity;
  title: string;
  summary?: string;
  accessory?: ReactNode;
  defaultOpen?: boolean;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);

  useEffect(() => {
    if (defaultOpen) {
      setOpen(true);
    }
  }, [defaultOpen]);

  return (
    <section className="inspector-section overflow-hidden">
      <button
        type="button"
        className="inspector-section-trigger flex min-h-10 w-full items-center gap-2 rounded-md px-1 py-2 text-left transition-colors hover:bg-surface-2/70 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-focus"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <ChevronRight className={cn("h-3.5 w-3.5 shrink-0 text-subtle transition-transform duration-200 ease-out", open && "rotate-90")} />
        <Icon className="h-3.5 w-3.5 shrink-0 text-subtle" />
        <h3 className="type-label-md min-w-0 flex-1 truncate text-ink">{title}</h3>
        {accessory ?? (summary ? <span className="type-body-sm min-w-0 max-w-[132px] truncate text-right text-muted">{summary}</span> : null)}
      </button>
      <div
        data-inspector-section-content
        aria-hidden={!open}
        inert={open ? undefined : true}
        className={cn(
          "inspector-section-content grid min-w-0 transition-[grid-template-rows,opacity] duration-200 ease-out",
          open ? "grid-rows-[1fr] opacity-100" : "grid-rows-[0fr] opacity-0"
        )}
      >
        {open ? (
          <div className="min-h-0 overflow-hidden">
            <div className="pb-3 pl-6 pr-1 pt-0.5">{children}</div>
          </div>
        ) : null}
      </div>
    </section>
  );
}

function KeyValue({ label, value, title, mono }: { label: string; value: string; title?: string; mono?: boolean }) {
  return (
    <div className="type-body-md grid min-w-0 grid-cols-[82px_minmax(0,1fr)] items-start gap-2 py-0.5">
      <span className="min-w-0 truncate text-muted">{label}</span>
      <span title={title ?? value} className={cn("min-w-0 truncate text-right text-ink", mono && "type-code-sm")}>
        {value}
      </span>
    </div>
  );
}

function compactPath(path: string) {
  const normalized = path.replaceAll("\\", "/");
  const parts = normalized.split("/").filter(Boolean);
  if (parts.length <= 2) {
    return path;
  }
  return `.../${parts.slice(-2).join("/")}`;
}
