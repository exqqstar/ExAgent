import { Blocks, Bug, CircleAlert, FileText, FolderPlus, X } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Button } from "@/components/ui/button";
import { Composer } from "@/components/Composer";
import { Inspector } from "@/components/Inspector";
import { GoalControl } from "@/components/GoalControl";
import { TranscriptList } from "@/components/TranscriptList";
import { setComposerValue } from "@/stores/workbenchStore";
import type { getWorkbenchState } from "@/stores/workbenchStore";
import { cn } from "@/lib/utils";

type WorkbenchState = ReturnType<typeof getWorkbenchState>;

export function ChatView({ state }: { state: WorkbenchState }) {
  if (state.compareView) {
    return <BranchCompareView state={state} compare={state.compareView} />;
  }

  const empty = !state.loading && state.transcript.length === 0;
  const activeSession = state.sessions.find((session) => session.id === state.activeSessionId);
  const forkDisabled =
    state.loading ||
    Boolean(state.activeTurnId) ||
    activeSession?.status === "running" ||
    activeSession?.status === "awaiting_approval";

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {state.error ? <ChatError message={state.error} /> : null}
      <div className="min-h-0 flex-1 px-2 py-2">
        <div className="grid h-full w-full grid-cols-1 gap-3 2xl:grid-cols-[minmax(0,1fr)_360px] 2xl:items-start">
          <div className="flex h-full min-h-0 min-w-0 flex-col">
            {empty ? (
              <NewSessionState state={state} />
            ) : (
              <>
                <ScrollArea className="min-h-0 flex-1">
                  <div className="mx-auto flex w-full max-w-[920px] flex-col gap-5 pb-5 pt-1">
                    <TranscriptList
                      messages={state.transcript}
                      loading={state.loading}
                      forkDisabled={forkDisabled}
                      onForkFromTurn={state.forkThreadFromTurn}
                      groupTurnActivity
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
            className="inspector-panel hidden h-fit max-h-[calc(100dvh-4rem)] w-full min-w-0 overflow-x-hidden overflow-y-auto rounded-xl border border-border 2xl:block"
            aria-label="Inspector"
          >
            <Inspector state={state} variant="panel" />
          </aside>
        </div>
      </div>
    </div>
  );
}

function BranchCompareView({
  state,
  compare
}: {
  state: WorkbenchState;
  compare: NonNullable<WorkbenchState["compareView"]>;
}) {
  const sharedTurnLabel =
    compare.sharedTurnCount === 1 ? "1 shared turn" : `${compare.sharedTurnCount} shared turns`;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {state.error ? <ChatError message={state.error} /> : null}
      <div className="min-h-0 flex-1 px-2 py-2">
        <div className="mx-auto flex h-full w-full max-w-[1280px] flex-col gap-3">
          <div className="flex items-center justify-between gap-3 rounded-lg border border-border bg-surface-1 px-4 py-3">
            <p className="type-label-md text-ink">{sharedTurnLabel}</p>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              aria-label="Close branch compare"
              onClick={state.closeCompareView}
            >
              <X className="h-4 w-4" />
            </Button>
          </div>
          {compare.error ? (
            <div role="alert" className="rounded-lg border border-danger/30 bg-danger/8 px-4 py-3 text-danger">
              <p className="type-body-sm break-words">{compare.error}</p>
            </div>
          ) : null}
          <div className="grid min-h-0 flex-1 grid-cols-1 gap-3 xl:grid-cols-2">
            <ComparePane
              label="Parent branch transcript"
              title={compare.parentTitle}
              eyebrow="Parent branch"
              messages={compare.parentTranscript}
              loading={compare.loading}
            />
            <ComparePane
              label="Fork branch transcript"
              title={compare.childTitle}
              eyebrow="Fork branch"
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
  return (
    <section aria-label={label} className="flex min-h-0 min-w-0 flex-col rounded-lg border border-border bg-surface-1">
      <div className="border-b border-border px-4 py-3">
        <p className="type-label-sm text-muted">{eyebrow}</p>
        <h2 className="type-title-md truncate text-ink">{title}</h2>
      </div>
      <ScrollArea className="min-h-0 flex-1">
        <div className="px-4 py-4">
          <TranscriptList
            messages={messages}
            loading={loading}
            emptyLabel="No post-fork turns in this branch."
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
  const project = state.projects.find((item) => item.id === state.activeProjectId);
  if (!project) {
    return <NoProjectState state={state} />;
  }
  const projectName = project.name;

  return (
    <div className="flex min-h-0 flex-1 items-center justify-center px-4 py-8">
      <div className="w-full max-w-[820px]">
        <div className="mb-8 text-center">
          <h2 className="type-empty-title text-ink">What should we build in {projectName}?</h2>
        </div>

        <Composer state={state} variant="hero" />
        <GoalControl state={state} variant="hero" />

        <div className="mt-7 grid gap-3 sm:grid-cols-[1.08fr_0.92fr]">
          {newSessionPrompts.map((prompt, index) => (
            <button
              key={prompt.title}
              type="button"
              className={cn(
                "prompt-card group rounded-lg p-4 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus",
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
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center px-4 py-8">
      <div className="w-full max-w-[520px] text-center">
        <div className="mx-auto flex h-10 w-10 items-center justify-center rounded-lg border border-border bg-surface-1 text-muted">
          <FolderPlus className="h-5 w-5" />
        </div>
        <h2 className="type-empty-title mt-5 text-ink">Add a project</h2>
        <p className="type-body-md mx-auto mt-2 max-w-[360px] text-muted">
          Choose a folder before starting a session.
        </p>
        <div className="mt-5 flex justify-center">
          <Button type="button" onClick={() => void state.addProject()}>
            <FolderPlus className="h-4 w-4" />
            <span>Add project</span>
          </Button>
        </div>
      </div>
    </div>
  );
}

const newSessionPrompts = [
  {
    icon: Blocks,
    title: "Build a feature",
    description: "Describe the product behavior you want",
    value: "Build "
  },
  {
    icon: Bug,
    title: "Fix a problem",
    description: "Point ExAgent at a bug or rough edge",
    value: "Fix "
  },
  {
    icon: FileText,
    title: "Review the code",
    description: "Ask for risks, regressions, and tests",
    value: "Review "
  }
];
