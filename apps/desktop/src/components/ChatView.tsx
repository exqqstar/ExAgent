import { Blocks, Bug, CircleAlert, FileText } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Composer } from "@/components/Composer";
import { Inspector } from "@/components/Inspector";
import { GoalControl } from "@/components/GoalControl";
import { TranscriptList } from "@/components/TranscriptList";
import { setComposerValue } from "@/stores/workbenchStore";
import type { getWorkbenchState } from "@/stores/workbenchStore";
import { cn } from "@/lib/utils";

type WorkbenchState = ReturnType<typeof getWorkbenchState>;

export function ChatView({ state }: { state: WorkbenchState }) {
  const empty = !state.loading && state.transcript.length === 0;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {state.error ? <ChatError message={state.error} /> : null}
      <div className="min-h-0 flex-1 px-2 py-2">
        <div className="grid h-full w-full grid-cols-1 gap-3 2xl:grid-cols-[minmax(0,1fr)_320px] 2xl:items-start">
          <div className="flex h-full min-h-0 min-w-0 flex-col">
            {empty ? (
              <NewSessionState state={state} />
            ) : (
              <>
                <ScrollArea className="min-h-0 flex-1">
                  <div className="mx-auto flex w-full max-w-[920px] flex-col gap-5 pb-5 pt-1">
                    <TranscriptList messages={state.transcript} loading={state.loading} />
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
            className="inspector-panel hidden h-fit max-h-[calc(100dvh-4rem)] min-w-0 overflow-x-hidden overflow-y-auto rounded-xl border border-border 2xl:block 2xl:justify-self-end"
            aria-label="Inspector"
          >
            <Inspector state={state} variant="panel" />
          </aside>
        </div>
      </div>
    </div>
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
  const projectName = project?.name ?? "ExAgent";

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
