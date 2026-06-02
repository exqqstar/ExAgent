import { Activity, Cpu, FileText, Gauge, HardDrive, ShieldCheck } from "lucide-react";
import type * as React from "react";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import type { getWorkbenchState } from "@/stores/workbenchStore";
import { cn } from "@/lib/utils";

type WorkbenchState = ReturnType<typeof getWorkbenchState>;

const eventVariant = {
  neutral: "neutral",
  info: "info",
  warning: "warning",
  danger: "danger",
  success: "success"
} as const;

export function Inspector({ state }: { state: WorkbenchState }) {
  const usagePercent = Math.min(100, Math.round(((state.tokenUsage.input + state.tokenUsage.output) / state.tokenUsage.limit) * 100));
  const activeSession = state.sessions.find((session) => session.id === state.activeSessionId);
  const activeStatus = activeSession?.status ?? "idle";
  const runtimeModel = state.selectedModel?.model_id ?? state.runtimeSettings?.default_model ?? "default";
  const thinkingMode = state.selectedThinkingMode ?? state.runtimeSettings?.default_thinking_mode ?? "auto";
  const enabledMcpServers = state.runtimeSettings?.mcp_servers.filter((server) => server.enabled).length ?? 0;
  const enabledSkillRoots = state.runtimeSettings?.skill_roots.filter((root) => root.enabled).length ?? 0;

  return (
    <div className="flex h-full flex-col">
      <div className="p-4">
        <h2 className="text-lg font-semibold text-ink">Inspector</h2>
        <p className="mt-1 text-sm text-muted">Runtime state and workspace summary</p>
      </div>
      <Separator />
      <ScrollArea className="min-h-0 flex-1">
        <div className="space-y-5 p-4">
          <InspectorSection icon={Activity} title="Progress">
            <div className="flex items-center justify-between gap-3">
              <span className="text-sm text-muted">Current turn</span>
              <Badge variant={activeStatus === "failed" ? "danger" : activeStatus === "awaiting_approval" ? "warning" : activeStatus === "running" ? "success" : "neutral"}>
                {activeStatus.replace("_", " ")}
              </Badge>
            </div>
            <div className="mt-3 h-1.5 rounded-full bg-surface-3">
              <div className={cn("h-full rounded-full bg-primary", activeStatus === "running" ? "w-2/3" : "w-0")} />
            </div>
          </InspectorSection>

          <InspectorSection icon={HardDrive} title="Environment">
            <KeyValue label="cwd" value={state.cwd} mono />
            <KeyValue label="policy" value={state.policy} />
          </InspectorSection>

          <InspectorSection icon={Cpu} title="Runtime">
            <KeyValue label="model" value={runtimeModel} mono />
            <KeyValue label="thinking" value={thinkingMode} />
            <KeyValue label="MCP servers" value={`${enabledMcpServers} enabled`} />
            <KeyValue label="Skill roots" value={`${enabledSkillRoots} enabled`} />
          </InspectorSection>

          <InspectorSection icon={Gauge} title="Token Usage">
            <KeyValue label="input" value={state.tokenUsage.input.toLocaleString()} mono />
            <KeyValue label="output" value={state.tokenUsage.output.toLocaleString()} mono />
            <div className="mt-3 flex items-center justify-between gap-3 text-sm">
              <span className="text-muted">context</span>
              <span className="font-mono text-ink">{usagePercent}%</span>
            </div>
          </InspectorSection>

          <InspectorSection icon={FileText} title="Changed Files">
            <div className="space-y-2">
              {state.changedFiles.length === 0 ? (
                <p className="text-sm text-muted">No changed files reported.</p>
              ) : (
                state.changedFiles.map((file) => (
                  <div key={file.path} className="flex items-center gap-2 rounded-md bg-surface-2 px-2 py-1.5">
                    <Badge variant={file.status === "deleted" ? "danger" : file.status === "added" ? "success" : "info"}>
                      {file.status}
                    </Badge>
                    <span className="min-w-0 truncate font-mono text-xs text-muted">{file.path}</span>
                  </div>
                ))
              )}
            </div>
          </InspectorSection>

          <InspectorSection icon={ShieldCheck} title="Events">
            <div className="space-y-3">
              {state.events.map((event) => (
                <div key={event.id} className="border-l border-border pl-3">
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-sm font-medium text-ink">{event.label}</span>
                    <Badge variant={eventVariant[event.tone ?? "neutral"]}>{event.timestamp}</Badge>
                  </div>
                  <p className="mt-1 text-sm leading-5 text-muted">{event.detail}</p>
                </div>
              ))}
            </div>
          </InspectorSection>
        </div>
      </ScrollArea>
    </div>
  );
}

function InspectorSection({
  icon: Icon,
  title,
  children
}: {
  icon: typeof Activity;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section>
      <div className="mb-2 flex items-center gap-2">
        <Icon className="h-4 w-4 text-subtle" />
        <h3 className="text-sm font-semibold text-ink">{title}</h3>
      </div>
      <div className="rounded-lg border border-border bg-surface-1 p-3">{children}</div>
    </section>
  );
}

function KeyValue({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex items-start justify-between gap-3 py-1 text-sm">
      <span className="shrink-0 text-muted">{label}</span>
      <span title={value} className={cn("min-w-0 flex-1 truncate text-right text-ink", mono && "font-mono text-xs")}>
        {value}
      </span>
    </div>
  );
}
