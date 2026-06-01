import { CheckCircle2, CircleAlert, Info, Terminal } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { ApprovalCard } from "@/components/ApprovalCard";
import { Composer } from "@/components/Composer";
import type { getWorkbenchState } from "@/stores/workbenchStore";
import type { TranscriptMessage } from "@/types";
import { cn } from "@/lib/utils";

type WorkbenchState = ReturnType<typeof getWorkbenchState>;

const roleLabel: Record<TranscriptMessage["role"], string> = {
  user: "You",
  assistant: "ExAgent",
  system: "System",
  tool: "Tool",
  approval: "Approval"
};

const statusIcon = {
  info: Info,
  success: CheckCircle2,
  warning: CircleAlert,
  danger: CircleAlert
};

export function ChatView({ state }: { state: WorkbenchState }) {
  return (
    <div className="flex min-h-0 flex-1 flex-col bg-bg">
      <ScrollArea className="min-h-0 flex-1">
        <div className="mx-auto flex w-full max-w-[920px] flex-col gap-3 px-4 py-5">
          {state.loading ? (
            <TranscriptSkeleton />
          ) : state.transcript.length === 0 ? (
            <div className="rounded-lg border border-border bg-surface-1 p-5">
              <h2 className="text-[22px] font-semibold text-ink">Start a session</h2>
              <p className="mt-2 max-w-xl text-sm leading-6 text-muted">
                Choose a project folder, restore a previous thread, or send the first prompt when the runtime is connected.
              </p>
            </div>
          ) : (
            state.transcript.map((message) => <TranscriptItem key={message.id} message={message} />)
          )}
        </div>
      </ScrollArea>

      <div className="border-t border-border bg-bg px-4 py-3">
        <div className="mx-auto max-w-[920px]">
          <Composer state={state} />
        </div>
      </div>
    </div>
  );
}

function TranscriptItem({ message }: { message: TranscriptMessage }) {
  if (message.role === "approval") {
    return <ApprovalCard message={message} />;
  }

  const Icon = message.status ? statusIcon[message.status] : message.role === "tool" ? Terminal : null;

  return (
    <article
      className={cn(
        "rounded-lg border border-border bg-surface-1 px-4 py-3",
        message.role === "user" && "bg-surface-2",
        message.role === "tool" && "border-border bg-surface-1"
      )}
    >
      <div className="flex items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-2">
          {Icon ? <Icon className="h-4 w-4 shrink-0 text-muted" /> : null}
          <span className="text-sm font-medium text-ink">{message.title ?? roleLabel[message.role]}</span>
          <Badge variant={message.status ?? "neutral"}>{roleLabel[message.role]}</Badge>
        </div>
        <time className="text-xs text-subtle">{message.timestamp}</time>
      </div>
      <p className="mt-2 whitespace-pre-wrap text-base leading-[1.55] text-muted">{message.body}</p>
    </article>
  );
}

function TranscriptSkeleton() {
  return (
    <div className="space-y-3" aria-label="Loading transcript">
      <Skeleton className="h-24" />
      <Skeleton className="h-32" />
      <Skeleton className="h-24" />
    </div>
  );
}
