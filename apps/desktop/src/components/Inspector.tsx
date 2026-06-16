import { Activity, Bot, ChevronRight, CircleAlert, Cpu, Database, FileText, Gauge, HardDrive, ShieldCheck } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { AgentsPanel } from "@/components/AgentsPanel";
import { TokenUsagePanel, tokenUsageSummary } from "@/components/TokenUsagePanel";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import { countAgents, countLiveAgents } from "@/lib/agentTree";
import type { getWorkbenchState } from "@/stores/workbenchStore";
import type { AgentNode } from "@/types";
import { useI18n, type TranslationKey } from "@/lib/i18n";
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
  const { t } = useI18n();
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
    liveAgents > 0
      ? t("inspector.agentSummary.running").replace("{count}", String(liveAgents))
      : totalAgents === 1
        ? t("inspector.agentSummary.singular")
        : t("inspector.agentSummary.plural").replace("{count}", String(totalAgents));
  const waitingApprovalAgents = agentRoot ? countWaitingApproval(agentRoot) : 0;
  const [expandWaitingSignal, setExpandWaitingSignal] = useState(0);

  const content = (
    <div className="inspector-sections p-3">
      <InspectorSection
        defaultOpen
        icon={Activity}
        title={t("inspector.sections.progress")}
        accessory={
          <Badge variant={activeStatus === "failed" ? "danger" : activeStatus === "awaiting_approval" ? "warning" : activeStatus === "running" ? "success" : "neutral"}>
            {sessionStatusLabel(activeStatus, t)}
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
          openSignal={expandWaitingSignal}
          title={t("inspector.sections.agents")}
          summary={agentSummary}
          accessory={
            waitingApprovalAgents > 0 ? (
              <WaitingApprovalHeaderButton
                count={waitingApprovalAgents}
                onClick={() => setExpandWaitingSignal((value) => value + 1)}
              />
            ) : null
          }
        >
          <AgentsPanel
            root={agentRoot}
            selectedThreadId={state.selectedAgentThreadId}
            onSelectAgent={state.openAgentThread}
            expandWaitingSignal={expandWaitingSignal}
          />
        </InspectorSection>
      ) : null}

      <InspectorSection icon={HardDrive} title={t("inspector.sections.environment")} summary={compactPath(state.cwd)}>
        <KeyValue label="cwd" value={compactPath(state.cwd)} title={state.cwd} mono />
        <KeyValue label="policy" value={state.policy} />
      </InspectorSection>

      <InspectorSection defaultOpen icon={Cpu} title={t("inspector.sections.runtime")} summary={runtimeModel}>
        <KeyValue label="model" value={runtimeModel} mono />
        <KeyValue label="thinking" value={thinkingMode} />
        <KeyValue label="MCP servers" value={t("inspector.enabledCount").replace("{count}", String(enabledMcpServers))} />
        <KeyValue label="Skill roots" value={t("inspector.enabledCount").replace("{count}", String(enabledSkillRoots))} />
      </InspectorSection>

      <InspectorSection
        defaultOpen
        icon={Gauge}
        title={t("inspector.sections.tokenUsage")}
        summary={tokenUsageSummary(rootTokenUsage, t)}
      >
        <TokenUsagePanel usage={rootTokenUsage} />
      </InspectorSection>

      <InspectorSection
        defaultOpen
        icon={Database}
        title={t("inspector.sections.contextWindow")}
        summary={contextPercent != null
          ? t("inspector.context.percentUsed").replace("{percent}", String(contextPercent))
          : t("inspector.context.notReported")}
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
          <p className="type-body-md text-muted">{t("inspector.context.empty")}</p>
        )}
      </InspectorSection>

      <InspectorSection
        icon={FileText}
        title={t("inspector.sections.changedFiles")}
        summary={state.changedFiles.length === 0
          ? t("inspector.changedFiles.none")
          : t("inspector.changedFiles.changed").replace("{count}", String(state.changedFiles.length))}
      >
        <div className="space-y-2">
          {state.changedFiles.length === 0 ? (
            <p className="type-body-md text-muted">{t("inspector.changedFiles.empty")}</p>
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

      <InspectorSection
        icon={ShieldCheck}
        title={t("inspector.sections.events")}
        summary={t("inspector.events.recorded").replace("{count}", String(state.events.length))}
      >
        <div className="space-y-2.5">
          {state.events.length === 0 ? (
            <p className="type-body-md text-muted">{t("inspector.events.empty")}</p>
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
  openSignal = 0,
  defaultOpen = false,
  children
}: {
  icon: typeof Activity;
  title: string;
  summary?: string;
  accessory?: ReactNode;
  openSignal?: number;
  defaultOpen?: boolean;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);

  useEffect(() => {
    if (defaultOpen) {
      setOpen(true);
    }
  }, [defaultOpen]);

  useEffect(() => {
    if (openSignal > 0) {
      setOpen(true);
    }
  }, [openSignal]);

  return (
    <section className="inspector-section overflow-hidden">
      <div className="inspector-section-trigger flex min-h-10 w-full items-center gap-2 rounded-md px-1 py-2 transition-colors hover:bg-surface-2/70">
        <button
          type="button"
          className="flex min-w-0 flex-1 items-center gap-2 rounded text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-focus"
          aria-expanded={open}
          onClick={() => setOpen((value) => !value)}
        >
          <ChevronRight className={cn("h-3.5 w-3.5 shrink-0 text-subtle transition-transform duration-200 ease-out", open && "rotate-90")} />
          <Icon className="h-3.5 w-3.5 shrink-0 text-subtle" />
          <h3 className="type-label-md min-w-0 flex-1 truncate text-ink">{title}</h3>
          {summary ? <span className="type-body-sm min-w-0 max-w-[132px] truncate text-right text-muted">{summary}</span> : null}
        </button>
        {accessory ? <div className="flex shrink-0 items-center">{accessory}</div> : null}
      </div>
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

function WaitingApprovalHeaderButton({ count, onClick }: { count: number; onClick: () => void }) {
  const { t } = useI18n();

  return (
    <button
      type="button"
      aria-label={
        count === 1
          ? t("inspector.waitingApproval.expandSingular")
          : t("inspector.waitingApproval.expandPlural").replace("{count}", String(count))
      }
      onClick={onClick}
      className="shrink-0 rounded focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-focus"
    >
      <Badge variant="warning" className="gap-1">
        <CircleAlert aria-hidden className="h-3 w-3 shrink-0" />
        <span>{count}</span>
        <span>
          {count === 1
            ? t("inspector.waitingApproval.approvalSingular")
            : t("inspector.waitingApproval.approvalPlural")}
        </span>
      </Badge>
    </button>
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

function countWaitingApproval(node: AgentNode): number {
  const self = node.status === "waiting_approval" ? 1 : 0;
  return self + node.children.reduce((count, child) => count + countWaitingApproval(child), 0);
}

function sessionStatusLabel(status: string, t: (key: TranslationKey) => string) {
  switch (status) {
    case "running":
      return t("status.session.running");
    case "awaiting_approval":
      return t("status.session.awaitingApproval");
    case "failed":
      return t("status.session.failed");
    case "archived":
      return t("status.session.archived");
    default:
      return t("status.session.idle");
  }
}
