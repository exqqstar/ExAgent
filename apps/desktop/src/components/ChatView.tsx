import { useEffect, useMemo, useRef } from "react";
import { Blocks, Bug, CircleAlert, FileText, FolderPlus, X } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Button } from "@/components/ui/button";
import { Composer } from "@/components/Composer";
import { Inspector } from "@/components/Inspector";
import { GoalControl } from "@/components/GoalControl";
import { TranscriptList } from "@/components/TranscriptList";
import { setComposerValue } from "@/stores/workbenchStore";
import type { getWorkbenchState } from "@/stores/workbenchStore";
import type { WorkflowRunView } from "@/types";
import { useI18n, type TranslationKey } from "@/lib/i18n";
import { cn } from "@/lib/utils";

type WorkbenchState = ReturnType<typeof getWorkbenchState>;

export function ChatView({ state }: { state: WorkbenchState }) {
  const { t } = useI18n();
  const transcriptScrollRef = useRef<HTMLDivElement | null>(null);

  const empty = !state.activeWorkflowRun && !state.loading && state.transcript.length === 0;
  const activeSession = state.sessions.find((session) => session.id === state.activeSessionId);
  const workflowRun = state.activeWorkflowRun;
  const activeRun =
    state.activeSessionId && (activeSession?.status === "running" || activeSession?.status === "awaiting_approval")
      ? {
          threadId: state.activeSessionId,
          turnId: state.activeTurnId ?? null,
          status: activeSession.status
        }
      : null;
  const activeRunScrollSignature = useMemo(() => {
    if (!activeRun) {
      return null;
    }
    const latest = state.transcript.at(-1);
    return [
      activeRun.threadId,
      activeRun.turnId ?? "pending",
      activeRun.status,
      state.transcript.length,
      latest?.id ?? "none",
      latest?.body.length ?? 0,
      latest?.toolStatus ?? "none",
      latest?.turnStatus ?? "none"
    ].join(":");
  }, [activeRun, state.transcript]);
  const forkDisabled =
    state.loading ||
    Boolean(state.activeTurnId) ||
    activeSession?.status === "running" ||
    activeSession?.status === "awaiting_approval";

  useEffect(() => {
    if (!activeRunScrollSignature) {
      return;
    }
    const viewport = transcriptScrollRef.current?.querySelector<HTMLElement>(
      "[data-radix-scroll-area-viewport]"
    );
    if (!viewport) {
      return;
    }
    const frame = window.requestAnimationFrame(() => {
      viewport.scrollTop = viewport.scrollHeight;
    });
    return () => window.cancelAnimationFrame(frame);
  }, [activeRunScrollSignature]);

  useEffect(() => {
    if (!workflowRun || !["queued", "running", "waiting_approval"].includes(workflowRun.status)) {
      return;
    }
    const interval = window.setInterval(() => {
      void state.refreshActiveWorkflowRun();
    }, 1000);
    return () => window.clearInterval(interval);
  }, [state, workflowRun?.run_id, workflowRun?.status]);

  if (state.compareView) {
    return <BranchCompareView state={state} compare={state.compareView} />;
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {state.error ? <ChatError message={state.error} /> : null}
      <div className="min-h-0 flex-1 px-3 py-3">
        <div className="grid h-full w-full grid-cols-1 gap-3 2xl:grid-cols-[minmax(0,1fr)_360px] 2xl:items-start">
          <div className="flex h-full min-h-0 min-w-0 flex-col">
            {empty ? (
              <NewSessionState state={state} />
            ) : (
              <>
                <ScrollArea ref={transcriptScrollRef} className="min-h-0 flex-1">
                  <div className="mx-auto flex w-full max-w-[920px] flex-col gap-5 pb-5 pt-1">
                    {workflowRun ? (
                      <WorkflowRunPanel run={workflowRun} onCancel={state.cancelActiveWorkflowRun} />
                    ) : null}
                    <TranscriptList
                      messages={state.transcript}
                      loading={state.loading && state.transcript.length === 0}
                      forkDisabled={forkDisabled}
                      onForkFromTurn={state.forkThreadFromTurn}
                      groupTurnActivity
                      activeRun={activeRun}
                    />
                  </div>
                </ScrollArea>

                <div className="pt-3">
                  <div className="mx-auto max-w-[920px]">
                    <GoalControl state={state} />
                    <Composer state={state} />
                  </div>
                </div>
              </>
            )}
          </div>

          <aside
            className="inspector-panel hidden h-fit max-h-[calc(100dvh-4rem)] w-full min-w-0 overflow-x-hidden overflow-y-auto rounded-2xl border border-border 2xl:block"
            aria-label={t("chat.inspector")}
          >
            <Inspector state={state} variant="panel" />
          </aside>
        </div>
      </div>
    </div>
  );
}

function WorkflowRunPanel({ run, onCancel }: { run: WorkflowRunView; onCancel: () => Promise<void> }) {
  const terminal = ["completed", "failed", "cancelled"].includes(run.status);
  const tone =
    run.status === "failed"
      ? "border-danger/30 bg-danger/8"
      : run.status === "completed"
        ? "border-success/30 bg-success/8"
        : "border-border bg-surface-1";
  const completed = run.phases.reduce((total, phase) => total + phase.completed_count, 0);
  const failed = run.phases.reduce((total, phase) => total + phase.failed_count, 0);
  const planned = run.phases.reduce((total, phase) => total + phase.planned_count, 0);

  return (
    <section className={cn("rounded-lg border px-4 py-3", tone)} aria-label="Workflow run">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
        <div className="min-w-0">
          <div className="flex min-w-0 items-center gap-2">
            <FileText className="h-4 w-4 shrink-0 text-muted" />
            <h2 className="type-title-md truncate text-ink">{run.label}</h2>
            <span className="type-label-sm rounded border border-border bg-surface-2 px-2 py-0.5 text-muted">
              {workflowStatusLabel(run.status)}
            </span>
          </div>
          {run.report_summary ? (
            <p className="type-body-sm mt-2 whitespace-pre-wrap text-muted">{run.report_summary}</p>
          ) : null}
        </div>
        {!terminal ? (
          <Button type="button" variant="outline" size="sm" onClick={() => void onCancel()}>
            <X className="h-4 w-4" />
            Cancel
          </Button>
        ) : null}
      </div>

      <div className="mt-3 grid gap-2 sm:grid-cols-3">
        <WorkflowMetric label="Agents" value={`${run.stats.agent_calls}`} />
        <WorkflowMetric label="Phase work" value={`${completed}/${planned}`} />
        <WorkflowMetric label="Failed" value={`${failed + run.stats.failed_agent_calls}`} />
      </div>

      <div className="mt-3 space-y-2">
        {run.phases.map((phase) => (
          <div key={phase.id} className="rounded-md border border-border bg-surface-2/55 px-3 py-2">
            <div className="flex items-center justify-between gap-3">
              <span className="type-label-md text-ink">{phase.label}</span>
              <span className="type-label-sm text-muted">{workflowStatusLabel(phase.status)}</span>
            </div>
            <div className="type-body-sm mt-1 text-muted">
              {phase.completed_count}/{phase.planned_count}
              {phase.failed_count > 0 ? ` · ${phase.failed_count} failed` : ""}
              {phase.skipped_count > 0 ? ` · ${phase.skipped_count} skipped` : ""}
            </div>
          </div>
        ))}
      </div>
    </section>
  );
}

function WorkflowMetric({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-md border border-border bg-surface-2/55 px-3 py-2">
      <p className="type-label-sm text-muted">{label}</p>
      <p className="type-title-sm text-ink">{value}</p>
    </div>
  );
}

function workflowStatusLabel(status: string) {
  return status.replace(/_/g, " ");
}

function BranchCompareView({
  state,
  compare
}: {
  state: WorkbenchState;
  compare: NonNullable<WorkbenchState["compareView"]>;
}) {
  const { t } = useI18n();
  const sharedTurnLabel =
    compare.sharedTurnCount === 1
      ? t("chat.compare.sharedTurnSingular")
      : t("chat.compare.sharedTurnPlural").replace("{count}", String(compare.sharedTurnCount));

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {state.error ? <ChatError message={state.error} /> : null}
      <div className="min-h-0 flex-1 px-3 py-3">
        <div className="mx-auto flex h-full w-full max-w-[1280px] flex-col gap-3">
          <div className="flex items-center justify-between gap-3 rounded-xl border border-border bg-surface-1 px-4 py-3">
            <p className="type-label-md text-ink">{sharedTurnLabel}</p>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              aria-label={t("chat.compare.close")}
              onClick={state.closeCompareView}
            >
              <X className="h-4 w-4" />
            </Button>
          </div>
          {compare.error ? (
            <div role="alert" className="rounded-xl border border-danger/30 bg-danger/8 px-4 py-3 text-danger">
              <p className="type-body-sm break-words">{compare.error}</p>
            </div>
          ) : null}
          <div className="grid min-h-0 flex-1 grid-cols-1 gap-3 xl:grid-cols-2">
            <ComparePane
              label={t("chat.compare.parentLabel")}
              title={compare.parentTitle}
              eyebrow={t("chat.compare.parentEyebrow")}
              messages={compare.parentTranscript}
              loading={compare.loading}
            />
            <ComparePane
              label={t("chat.compare.forkLabel")}
              title={compare.childTitle}
              eyebrow={t("chat.compare.forkEyebrow")}
              messages={compare.childTranscript}
              loading={compare.loading}
            />
          </div>
        </div>
      </div>
    </div>
  );
}

function ComparePane({
  label,
  title,
  eyebrow,
  messages,
  loading
}: {
  label: string;
  title: string;
  eyebrow: string;
  messages: WorkbenchState["transcript"];
  loading: boolean;
}) {
  const { t } = useI18n();

  return (
    <section aria-label={label} className="flex min-h-0 min-w-0 flex-col rounded-xl border border-border bg-surface-1">
      <div className="border-b border-border px-4 py-3">
        <p className="type-label-sm text-muted">{eyebrow}</p>
        <h2 className="type-title-md truncate text-ink">{title}</h2>
      </div>
      <ScrollArea className="min-h-0 flex-1">
        <div className="px-4 py-4">
          <TranscriptList
            messages={messages}
            loading={loading}
            emptyLabel={t("chat.compare.empty")}
            readOnly
            groupTurnActivity
          />
        </div>
      </ScrollArea>
    </section>
  );
}

function ChatError({ message }: { message: string }) {
  return (
    <div className="border-b border-border bg-danger/8 px-4 py-3">
      <div className="mx-auto flex max-w-[920px] items-start gap-2 text-danger">
        <CircleAlert className="mt-0.5 h-4 w-4 shrink-0" />
        <p className="type-body-sm break-words">{message}</p>
      </div>
    </div>
  );
}

function NewSessionState({ state }: { state: WorkbenchState }) {
  const { t } = useI18n();
  const project = state.projects.find((item) => item.id === state.activeProjectId);
  if (!project) {
    return <NoProjectState state={state} />;
  }
  const projectName = project.name;
  const prompts = newSessionPrompts(t);

  return (
    <div className="flex min-h-0 flex-1 items-center justify-center px-4 py-8">
      <div className="w-full max-w-[820px]">
        <div className="mb-8 text-center">
          <h2 className="type-empty-title text-ink">
            {t("chat.empty.title").replace("{project}", projectName)}
          </h2>
        </div>

        <Composer state={state} variant="hero" />
        <GoalControl state={state} variant="hero" />

        <div className="mt-7 grid gap-3 sm:grid-cols-[1.08fr_0.92fr]">
          {prompts.map((prompt, index) => (
            <button
              key={prompt.title}
              type="button"
              className={cn(
                "prompt-card group rounded-xl p-4 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus",
                index === 0 ? "min-h-[150px] sm:row-span-2" : "min-h-[92px]"
              )}
              onClick={() => setComposerValue(prompt.value)}
            >
              <prompt.icon
                className={cn(
                  "text-muted transition-colors group-hover:text-primary-hover",
                  index === 0 ? "h-5 w-5" : "h-4 w-4"
                )}
              />
              <div className={cn("type-label-md text-ink", index === 0 ? "mt-8" : "mt-3")}>{prompt.title}</div>
              <div className="type-body-sm mt-1 text-muted">{prompt.description}</div>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

function NoProjectState({ state }: { state: WorkbenchState }) {
  const { t } = useI18n();

  return (
    <div className="flex min-h-0 flex-1 items-center justify-center px-4 py-8">
      <div className="w-full max-w-[520px] text-center">
        <div className="mx-auto flex h-10 w-10 items-center justify-center rounded-xl border border-border bg-surface-1 text-muted">
          <FolderPlus className="h-5 w-5" />
        </div>
        <h2 className="type-empty-title mt-5 text-ink">{t("chat.empty.addProject.title")}</h2>
        <p className="type-body-md mx-auto mt-2 max-w-[360px] text-muted">
          {t("chat.empty.addProject.description")}
        </p>
        <div className="mt-5 flex justify-center">
          <Button type="button" onClick={() => void state.addProject()}>
            <FolderPlus className="h-4 w-4" />
            <span>{t("chat.empty.addProject.action")}</span>
          </Button>
        </div>
      </div>
    </div>
  );
}

function newSessionPrompts(t: (key: TranslationKey) => string) {
  return [
    {
      icon: Blocks,
      title: t("chat.prompt.buildFeature.title"),
      description: t("chat.prompt.buildFeature.description"),
      value: t("chat.prompt.buildFeature.value")
    },
    {
      icon: Bug,
      title: t("chat.prompt.fixProblem.title"),
      description: t("chat.prompt.fixProblem.description"),
      value: t("chat.prompt.fixProblem.value")
    },
    {
      icon: FileText,
      title: t("chat.prompt.reviewCode.title"),
      description: t("chat.prompt.reviewCode.description"),
      value: t("chat.prompt.reviewCode.value")
    }
  ];
}
