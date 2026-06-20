import {
  CheckCircle2,
  ChevronRight,
  CircleAlert,
  Clock3,
  Copy,
  FileText,
  GitBranchPlus,
  Info,
  LoaderCircle,
  Terminal
} from "lucide-react";
import { memo, useEffect, useState } from "react";
import { ApprovalCard } from "@/components/ApprovalCard";
import { Markdown } from "@/components/Markdown";
import { QuestionCard } from "@/components/QuestionCard";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { SessionStatus, TranscriptMessage, TurnInput } from "@/types";
import { useI18n } from "@/lib/i18n";
import { fileBaseName, localFileAssetSrc } from "@/lib/media";
import { cn } from "@/lib/utils";

const roleLabel: Record<TranscriptMessage["role"], string> = {
  user: "You",
  assistant: "ExAgent",
  reasoning: "Reasoning",
  system: "System",
  tool: "Tool",
  approval: "Approval",
  goal_report: "Goal report"
};

const statusIcon = {
  info: Info,
  success: CheckCircle2,
  warning: CircleAlert,
  danger: CircleAlert
};

const toolStatusLabel: Record<NonNullable<TranscriptMessage["toolStatus"]>, string> = {
  running: "Running",
  waiting_approval: "Waiting approval",
  waiting_user_input: "Waiting input",
  completed: "Completed",
  failed: "Failed",
  cancelled: "Cancelled"
};

type TranscriptRenderItem =
  | { type: "message"; message: TranscriptMessage }
  | {
      type: "activity";
      id: string;
      threadId?: string;
      turnId?: string;
      turnStatus?: string;
      messages: TranscriptMessage[];
      defaultExpanded: boolean;
      active: boolean;
      activeStatus?: ActiveRunStatus;
    };

const activityRoles = new Set<TranscriptMessage["role"]>(["reasoning", "tool", "approval"]);
type ActiveRunStatus = Extract<SessionStatus, "running" | "awaiting_approval">;
type ActiveRun = {
  threadId: string;
  turnId: string | null;
  status: ActiveRunStatus;
};

function transcriptRenderItems(
  messages: TranscriptMessage[],
  groupTurnActivity: boolean,
  activeRun?: ActiveRun | null
): TranscriptRenderItem[] {
  if (!groupTurnActivity) {
    return messages.map((message) => ({ type: "message", message }));
  }

  const result: TranscriptRenderItem[] = [];
  let index = 0;

  while (index < messages.length) {
    const message = messages[index];
    if (!message.threadId || !message.turnId) {
      result.push({ type: "message", message });
      index += 1;
      continue;
    }

    const turnMessages: TranscriptMessage[] = [];
    const { threadId, turnId } = message;
    while (
      index < messages.length &&
      messages[index].threadId === threadId &&
      messages[index].turnId === turnId
    ) {
      turnMessages.push(messages[index]);
      index += 1;
    }

    result.push(...renderItemsForTurn(threadId, turnId, turnMessages, activeRun));
  }

  return insertActiveRunPlaceholder(result, activeRun);
}

function renderItemsForTurn(
  threadId: string,
  turnId: string,
  messages: TranscriptMessage[],
  activeRun?: ActiveRun | null
): TranscriptRenderItem[] {
  const activity = messages.filter((message) => activityRoles.has(message.role));
  const visible = messages.filter((message) => !activityRoles.has(message.role));

  if (activity.length === 0) {
    return messages.map((message) => ({ type: "message", message }));
  }

  const firstActivityIndex = messages.findIndex((message) => activityRoles.has(message.role));
  const firstVisibleAfterActivity = visible.findIndex((message) => messages.indexOf(message) > firstActivityIndex);
  const insertIndex = firstVisibleAfterActivity === -1 ? visible.length : firstVisibleAfterActivity;
  const turnStatus = messages.find((message) => message.turnStatus)?.turnStatus;
  const hasFinalAssistant = messages.some(isFinalAssistantMessage);
  const hasActiveTool = activity.some(
    (message) =>
      message.toolStatus === "running" ||
      message.toolStatus === "waiting_approval" ||
      message.toolStatus === "waiting_user_input"
  );
  const active = !hasFinalAssistant && activeRunMatches(activeRun, threadId, turnId);
  const group: TranscriptRenderItem = {
    type: "activity",
    id: `activity-${threadId}-${turnId}`,
    threadId,
    turnId,
    turnStatus,
    messages: activity,
    defaultExpanded: active || hasActiveTool || !hasFinalAssistant,
    active,
    activeStatus: active ? activeRun?.status : undefined
  };
  const items: TranscriptRenderItem[] = visible.map((message) => ({ type: "message", message }));
  items.splice(insertIndex, 0, group);
  return items;
}

function insertActiveRunPlaceholder(
  items: TranscriptRenderItem[],
  activeRun?: ActiveRun | null
): TranscriptRenderItem[] {
  if (!activeRun || items.some((item) => item.type === "activity" && item.active)) {
    return items;
  }

  const insertAfterIndex = lastActiveRunMessageIndex(items, activeRun);
  if (insertAfterIndex === -1) {
    return items;
  }

  const next = [...items];
  next.splice(insertAfterIndex + 1, 0, {
    type: "activity",
    id: `activity-${activeRun.threadId}-${activeRun.turnId ?? "pending"}`,
    threadId: activeRun.threadId,
    turnId: activeRun.turnId ?? undefined,
    messages: [],
    defaultExpanded: true,
    active: true,
    activeStatus: activeRun.status
  });
  return next;
}

function lastActiveRunMessageIndex(items: TranscriptRenderItem[], activeRun: ActiveRun) {
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index];
    if (item.type !== "message") {
      continue;
    }
    const message = item.message;
    if (!messageMatchesActiveRun(message, activeRun)) {
      continue;
    }
    if (isFinalAssistantMessage(message)) {
      return -1;
    }
    if (message.role !== "user") {
      continue;
    }
    return index;
  }
  return -1;
}

function messageMatchesActiveRun(message: TranscriptMessage, activeRun: ActiveRun) {
  if (message.threadId !== activeRun.threadId) {
    return false;
  }
  return activeRun.turnId ? message.turnId === activeRun.turnId : true;
}

function activeRunMatches(activeRun: ActiveRun | null | undefined, threadId: string, turnId: string) {
  if (!activeRun || activeRun.threadId !== threadId) {
    return false;
  }
  return activeRun.turnId ? activeRun.turnId === turnId : true;
}

function isFinalAssistantMessage(message: TranscriptMessage) {
  return (
    message.role === "assistant" &&
    message.body.trim().length > 0 &&
    !message.id.startsWith("stream-assistant-")
  );
}

export function TranscriptList({
  messages,
  loading = false,
  emptyLabel = "No transcript yet.",
  className,
  forkDisabled = false,
  readOnly = false,
  groupTurnActivity = false,
  activeRun,
  onForkFromTurn
}: {
  messages: TranscriptMessage[];
  loading?: boolean;
  emptyLabel?: string;
  className?: string;
  forkDisabled?: boolean;
  readOnly?: boolean;
  groupTurnActivity?: boolean;
  activeRun?: ActiveRun | null;
  onForkFromTurn?: (threadId: string, turnId: string) => void;
}) {
  const { t } = useI18n();

  if (loading) {
    return <TranscriptSkeleton />;
  }

  if (messages.length === 0) {
    return <p className="type-body-md text-muted">{emptyLabel}</p>;
  }

  const renderItems = transcriptRenderItems(messages, groupTurnActivity, activeRun);

  return (
    <div className={cn("flex flex-col gap-7", className)}>
      {renderItems.map((item) =>
        item.type === "activity" ? (
          <TurnActivityGroup key={item.id} group={item} readOnly={readOnly} />
        ) : (
          <TranscriptItem
            key={item.message.id}
            message={item.message}
            forkDisabled={forkDisabled}
            forkLabel={t("transcript.actions.forkFromReply")}
            readOnly={readOnly}
            onForkFromTurn={onForkFromTurn}
          />
        )
      )}
    </div>
  );
}

function TranscriptItemBase({
  message,
  forkDisabled = false,
  forkLabel = "Fork from here",
  readOnly = false,
  onForkFromTurn
}: {
  message: TranscriptMessage;
  forkDisabled?: boolean;
  forkLabel?: string;
  readOnly?: boolean;
  onForkFromTurn?: (threadId: string, turnId: string) => void;
}) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) {
      return;
    }
    const reset = window.setTimeout(() => setCopied(false), 1400);
    return () => window.clearTimeout(reset);
  }, [copied]);

  if (message.role === "approval") {
    return <ApprovalCard message={message} readOnly={readOnly} />;
  }

  if (message.toolStatus === "waiting_user_input" || message.requestId) {
    return <QuestionCard message={message} readOnly={readOnly} />;
  }

  if (message.role === "user") {
    const images = imageInputs(message.input);
    const hasBody = message.body.trim().length > 0;
    return (
      <article className="group flex items-start justify-end gap-2" aria-label="User message">
        <div className="user-bubble max-w-[min(74%,680px)] rounded-lg px-4 py-2.5 text-ink sm:max-w-[min(68%,680px)]">
          {hasBody ? <p className="type-body-lg whitespace-pre-wrap break-words">{message.body}</p> : null}
          {images.length > 0 ? <UserImageGrid images={images} hasBody={hasBody} /> : null}
        </div>
      </article>
    );
  }

  if (message.role === "assistant") {
    const canFork = Boolean(
      !readOnly && onForkFromTurn && message.threadId && message.turnId && message.turnStatus === "completed"
    );
    const canCopy = !readOnly && message.body.trim().length > 0;
    const showActions = canCopy || canFork;
    const copyLabel = copied ? t("transcript.actions.copiedReply") : t("transcript.actions.copyReply");
    return (
      <article className="group flex w-full max-w-[780px] flex-col py-1" aria-label="Assistant message">
        <div className="min-w-0 type-body-lg break-words text-muted">
          <Markdown content={message.body} />
        </div>
        {showActions ? (
          <div className="mt-2 flex items-center gap-1 text-subtle opacity-70 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
            {canCopy ? (
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    aria-label={copyLabel}
                    className="h-7 w-7 rounded-md"
                    onClick={() => {
                      void copyTranscriptText(message.body)
                        .then(() => setCopied(true))
                        .catch(() => undefined);
                    }}
                  >
                    <Copy className="h-3.5 w-3.5" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>{copyLabel}</TooltipContent>
              </Tooltip>
            ) : null}
            {canFork && message.threadId && message.turnId ? (
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    disabled={forkDisabled}
                    aria-label={forkLabel}
                    className="h-7 w-7 rounded-md"
                    onClick={() => onForkFromTurn?.(message.threadId as string, message.turnId as string)}
                  >
                    <GitBranchPlus className="h-3.5 w-3.5" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>{forkLabel}</TooltipContent>
              </Tooltip>
            ) : null}
          </div>
        ) : null}
      </article>
    );
  }

  if (message.role === "reasoning") {
    return <ReasoningBlock message={message} />;
  }

  if (message.role === "goal_report") {
    return <GoalReportCard message={message} />;
  }

  const Icon = message.status ? statusIcon[message.status] : message.role === "tool" ? Terminal : null;
  const badgeLabel = message.toolStatus ? toolStatusLabel[message.toolStatus] : roleLabel[message.role];
  const isToolOutput = message.role === "tool" && message.invocationId && message.body.trim().length > 0;

  return (
    <article
      className={cn(
        "message-card rounded-lg border border-border px-4 py-3",
        message.role === "tool" && "border-border"
      )}
    >
      <div className="flex items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-2">
          {Icon ? <Icon className="h-4 w-4 shrink-0 text-muted" /> : null}
          <span className="type-label-md text-ink">{message.title ?? roleLabel[message.role]}</span>
          <Badge variant={message.status ?? "neutral"}>{badgeLabel}</Badge>
          {message.mutating ? <Badge variant="warning">Mutating</Badge> : null}
        </div>
        <time className="type-label-sm text-subtle">{message.timestamp}</time>
      </div>
      {isToolOutput ? (
        <pre className="type-code-sm mt-2 max-h-48 overflow-auto whitespace-pre-wrap rounded border border-border bg-surface-2/80 px-3 py-2 text-muted">
          {message.body}
        </pre>
      ) : (
        <p className="type-body-lg mt-2 whitespace-pre-wrap text-muted">{message.body}</p>
      )}
    </article>
  );
}

// Memoized so a streaming message (whose `message` object changes each token)
// re-renders alone instead of re-parsing every other message's markdown.
// Callback identity is intentionally ignored — only data props gate re-render.
export const TranscriptItem = memo(
  TranscriptItemBase,
  (prev, next) =>
    prev.message === next.message &&
    prev.forkDisabled === next.forkDisabled &&
    prev.forkLabel === next.forkLabel &&
    prev.readOnly === next.readOnly
);

function TurnActivityGroupBase({
  group,
  readOnly
}: {
  group: Extract<TranscriptRenderItem, { type: "activity" }>;
  readOnly: boolean;
}) {
  const [expanded, setExpanded] = useState(group.defaultExpanded);
  const [manuallyChanged, setManuallyChanged] = useState(false);
  const summary = group.active ? activeActivitySummary(group.messages, group.activeStatus) : activitySummary(group.messages);
  const label = group.active ? "Working" : "Activity";

  useEffect(() => {
    if (!manuallyChanged) {
      setExpanded(group.defaultExpanded);
    }
  }, [group.defaultExpanded, manuallyChanged]);

  return (
    <section className="w-full max-w-[780px]" aria-label="Turn activity">
      <button
        type="button"
        className="group flex min-h-8 w-full items-center gap-2 rounded-md py-1 text-left text-muted transition-colors hover:text-ink focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus"
        aria-expanded={expanded}
        onClick={() => {
          setManuallyChanged(true);
          setExpanded((value) => !value);
        }}
      >
        <ChevronRight className={cn("h-4 w-4 shrink-0 transition-transform", expanded && "rotate-90")} />
        <span className={cn("type-label-md", group.active ? "text-ink" : "text-muted")}>{label}</span>
        <span className="type-label-sm min-w-0 truncate text-subtle">{summary}</span>
        {group.active ? (
          <Badge variant={group.activeStatus === "awaiting_approval" ? "warning" : "success"} className="shrink-0">
            {group.activeStatus === "awaiting_approval" ? "Waiting" : "Running"}
          </Badge>
        ) : null}
      </button>
      {expanded ? (
        <div className="mt-2 space-y-2 border-l border-border pl-4">
          {group.messages.length > 0 ? (
            group.messages.map((message) => (
              <TurnActivityMessage key={message.id} message={message} readOnly={readOnly} />
            ))
          ) : (
            <ActiveRunPlaceholder status={group.activeStatus ?? "running"} />
          )}
        </div>
      ) : null}
    </section>
  );
}

// Memoized so reasoning streaming in one activity group doesn't re-render
// (and re-parse markdown for) every other group in the transcript. The group
// wrapper is rebuilt every render, so compare by fields + message references
// (messages are immutable, so a changed message has a new reference).
function sameMessageRefs(a: TranscriptMessage[], b: TranscriptMessage[]) {
  return a.length === b.length && a.every((message, index) => message === b[index]);
}

const TurnActivityGroup = memo(
  TurnActivityGroupBase,
  (prev, next) =>
    prev.readOnly === next.readOnly &&
    prev.group.id === next.group.id &&
    prev.group.active === next.group.active &&
    prev.group.activeStatus === next.group.activeStatus &&
    prev.group.defaultExpanded === next.group.defaultExpanded &&
    sameMessageRefs(prev.group.messages, next.group.messages)
);

function ActiveRunPlaceholder({ status }: { status: ActiveRunStatus }) {
  const waitingApproval = status === "awaiting_approval";
  return (
    <div className="min-w-0 rounded-md border border-border bg-surface-1 px-3 py-2">
      <div className="flex min-w-0 items-center gap-2">
        <LoaderCircle className="h-3.5 w-3.5 shrink-0 animate-spin text-primary" />
        <span className="type-label-md min-w-0 truncate text-ink">
          {waitingApproval ? "waiting for approval" : "starting turn"}
        </span>
      </div>
      <p className="type-body-sm mt-1 text-muted">
        {waitingApproval ? "Waiting for an approval request to arrive." : "Waiting for first runtime event."}
      </p>
    </div>
  );
}

function TurnActivityMessage({ message, readOnly }: { message: TranscriptMessage; readOnly: boolean }) {
  if (message.role === "reasoning") {
    const body = message.body.trim();
    return (
      <div className="min-w-0">
        <div className="flex min-w-0 items-center gap-2">
          <Info className="h-3.5 w-3.5 shrink-0 text-subtle" />
          <span className="type-label-md text-ink">{message.title ?? "Reasoning"}</span>
        </div>
        {body ? (
          <div className="type-body-md mt-1 text-muted">
            <Markdown content={body} />
          </div>
        ) : null}
      </div>
    );
  }

  if (message.role === "approval") {
    return <ApprovalCard message={message} readOnly={readOnly} />;
  }

  const Icon = message.status ? statusIcon[message.status] : Terminal;
  const badgeLabel = message.toolStatus ? toolStatusLabel[message.toolStatus] : roleLabel[message.role];
  const isToolOutput = message.role === "tool" && message.invocationId && message.body.trim().length > 0;

  return (
    <div className="min-w-0 rounded-md border border-border bg-surface-1 px-3 py-2">
      <div className="flex min-w-0 items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-2">
          <Icon className="h-3.5 w-3.5 shrink-0 text-muted" />
          <span className="type-label-md min-w-0 truncate text-ink">{message.title ?? roleLabel[message.role]}</span>
          <Badge variant={message.status ?? "neutral"}>{badgeLabel}</Badge>
          {message.mutating ? <Badge variant="warning">Mutating</Badge> : null}
        </div>
        <time className="type-label-sm shrink-0 text-subtle">{message.timestamp}</time>
      </div>
      {message.body.trim().length > 0 ? (
        isToolOutput ? (
          <pre className="type-code-sm mt-2 max-h-48 overflow-auto whitespace-pre-wrap rounded border border-border bg-surface-2/80 px-3 py-2 text-muted">
            {message.body}
          </pre>
        ) : (
          <p className="type-body-md mt-2 whitespace-pre-wrap break-words text-muted">{message.body}</p>
        )
      ) : null}
    </div>
  );
}

function activitySummary(messages: TranscriptMessage[]) {
  const reasoningCount = messages.filter((message) => message.role === "reasoning").length;
  const toolCount = messages.filter((message) => message.role === "tool").length;
  const approvalCount = messages.filter((message) => message.role === "approval").length;
  const parts = [
    reasoningCount > 0 ? `${reasoningCount} reasoning` : null,
    toolCount > 0 ? `${toolCount} ${toolCount === 1 ? "tool" : "tools"}` : null,
    approvalCount > 0 ? `${approvalCount} ${approvalCount === 1 ? "approval" : "approvals"}` : null
  ].filter(Boolean);
  return parts.join(" · ");
}

function activeActivitySummary(messages: TranscriptMessage[], status: ActiveRunStatus | undefined) {
  const waitingInput = latestToolMessageWithStatus(messages, "waiting_user_input");
  if (waitingInput) {
    return "waiting for input";
  }
  const waitingApproval = latestToolMessageWithStatus(messages, "waiting_approval");
  if (waitingApproval || status === "awaiting_approval") {
    return "waiting for approval";
  }
  const runningTool = latestToolMessageWithStatus(messages, "running");
  if (runningTool) {
    return `running ${toolActivityLabel(runningTool)}`;
  }
  const latest = messages.at(-1);
  if (latest?.role === "reasoning") {
    return "thinking";
  }
  if (latest?.role === "tool" && latest.toolStatus === "completed") {
    return `completed ${toolActivityLabel(latest)}`;
  }
  return "starting turn";
}

function latestToolMessageWithStatus(
  messages: TranscriptMessage[],
  status: NonNullable<TranscriptMessage["toolStatus"]>
) {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message.role === "tool" && message.toolStatus === status) {
      return message;
    }
  }
  return null;
}

function toolActivityLabel(message: TranscriptMessage) {
  return message.toolName ?? message.title ?? "tool";
}

function GoalReportCard({ message }: { message: TranscriptMessage }) {
  const report = message.goalReport;
  if (!report) {
    return null;
  }
  const approvalsLabel = `${report.pending_approvals_count} ${report.pending_approvals_count === 1 ? "approval" : "approvals"} waiting in Inbox`;
  const changedFiles = report.changed_files ?? [];
  const openQuestions = report.open_questions ?? [];
  const reviewSummary = report.review_summary ?? null;

  return (
    <article className="message-card w-full max-w-[780px] rounded-lg border border-border px-4 py-3" aria-label="Goal report">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-2">
          <CheckCircle2 className="h-4 w-4 shrink-0 text-muted" />
          <span className="type-label-md text-ink">{message.title ?? "Goal report"}</span>
          <Badge variant={message.status ?? "neutral"}>{goalReportStatusLabel(report.final_status)}</Badge>
        </div>
        <time className="type-label-sm text-subtle">{message.timestamp}</time>
      </div>
      <div className="mt-3 space-y-2">
        <p className="type-label-md text-ink">{report.objective}</p>
        <p className="type-body-md whitespace-pre-wrap text-muted">{report.summary || message.body}</p>
      </div>
      <div className="mt-3 grid gap-2 sm:grid-cols-3">
        <GoalReportMetric label="Turns" value={`${report.turns_run} ${report.turns_run === 1 ? "turn" : "turns"}`} />
        <GoalReportMetric label="Tokens" value={tokenUsageValue(report.tokens_used, report.token_budget)} />
        <GoalReportMetric label="Time" value={durationValue(report.time_used_seconds)} />
      </div>
      {changedFiles.length > 0 ? (
        <div className="mt-3">
          <div className="flex items-center gap-2 text-muted">
            <FileText className="h-4 w-4" />
            <span className="type-label-sm">Changed files</span>
          </div>
          <ul className="mt-2 space-y-1">
            {changedFiles.map((file) => (
              <li key={file} className="type-code-sm break-all rounded border border-border bg-surface-2 px-2 py-1 text-muted">
                {file}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {openQuestions.length > 0 ? (
        <div className="mt-3">
          <div className="flex items-center gap-2 text-muted">
            <CircleAlert className="h-4 w-4" />
            <span className="type-label-sm">Open questions</span>
          </div>
          <ul className="mt-2 space-y-1">
            {openQuestions.map((question) => (
              <li
                key={question.question_id}
                className="rounded border border-border bg-surface-2 px-2 py-1.5 text-muted"
              >
                <p className="type-label-sm text-ink">{question.question}</p>
                <p className="type-body-sm mt-0.5">{question.blocks_what}</p>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {reviewSummary ? (
        <div className="mt-3 rounded-md border border-border bg-surface-2 px-3 py-2">
          <div className="flex flex-wrap items-center gap-2">
            <Info className="h-4 w-4 text-muted" />
            <span className="type-label-sm text-muted">Latest review</span>
            <Badge variant={reviewSummary.status === "approved" ? "success" : reviewSummary.status === "rejected" ? "danger" : "neutral"}>
              {reviewSummary.status}
            </Badge>
            {reviewSummary.reject_category ? (
              <Badge variant="warning">{reviewSummary.reject_category}</Badge>
            ) : null}
          </div>
          {reviewSummary.findings ? (
            <p className="type-body-sm mt-2 whitespace-pre-wrap text-muted">{reviewSummary.findings}</p>
          ) : null}
        </div>
      ) : null}
      {report.pending_approvals_count > 0 ? (
        <p className="type-label-md mt-3 inline-flex items-center gap-2 rounded-md border border-border px-3 py-1.5 text-ink">
          <Clock3 className="h-4 w-4 text-muted" />
          {approvalsLabel}
        </p>
      ) : null}
    </article>
  );
}

function GoalReportMetric({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-md border border-border bg-surface-2 px-3 py-2">
      <p className="type-label-sm text-subtle">{label}</p>
      <p className="type-label-md mt-1 text-ink">{value}</p>
    </div>
  );
}

function goalReportStatusLabel(status: NonNullable<TranscriptMessage["goalReport"]>["final_status"]) {
  return status.replace(/_/g, " ");
}

function tokenUsageValue(tokensUsed: number, tokenBudget?: number | null) {
  const used = numberFormatter.format(tokensUsed);
  return tokenBudget === null || tokenBudget === undefined
    ? `${used} tokens`
    : `${used} / ${numberFormatter.format(tokenBudget)} tokens`;
}

function durationValue(seconds: number) {
  const safeSeconds = Math.max(0, Math.floor(seconds));
  const minutes = Math.floor(safeSeconds / 60);
  const remainingSeconds = safeSeconds % 60;
  if (minutes === 0) {
    return `${remainingSeconds}s`;
  }
  if (remainingSeconds === 0) {
    return `${minutes}m`;
  }
  return `${minutes}m ${remainingSeconds}s`;
}

const numberFormatter = new Intl.NumberFormat();

async function copyTranscriptText(text: string) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }

  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "");
  textarea.style.position = "fixed";
  textarea.style.top = "-1000px";
  document.body.appendChild(textarea);
  textarea.select();
  try {
    document.execCommand("copy");
  } finally {
    document.body.removeChild(textarea);
  }
}

function UserImageGrid({
  images,
  hasBody
}: {
  images: Array<Extract<TurnInput, { type: "local_image" | "image_url" }>>;
  hasBody: boolean;
}) {
  return (
    <div className={cn("grid grid-cols-2 gap-2", hasBody && "mt-2")}>
      {images.map((image, index) => (
        <div
          key={`${image.type}-${imageKey(image)}-${index}`}
          className="min-w-0 overflow-hidden rounded-md border border-border bg-surface-2"
        >
          <TranscriptImagePreview image={image} />
          {image.type === "local_image" ? (
            <div className="type-label-sm truncate px-2 py-1 text-subtle">{fileBaseName(image.path)}</div>
          ) : null}
        </div>
      ))}
    </div>
  );
}

function TranscriptImagePreview({ image }: { image: Extract<TurnInput, { type: "local_image" | "image_url" }> }) {
  const [failed, setFailed] = useState(false);
  const label = image.type === "local_image" ? fileBaseName(image.path) : "Attached image";
  if (failed) {
    return (
      <div className="flex aspect-video w-full items-center justify-center bg-surface-3 text-subtle">
        <Info className="h-4 w-4" />
      </div>
    );
  }

  return (
    <img
      src={image.type === "local_image" ? localFileAssetSrc(image.path) : image.url}
      alt={label}
      loading="lazy"
      decoding="async"
      className="aspect-video w-full bg-surface-3 object-cover"
      onError={() => setFailed(true)}
    />
  );
}

function imageInputs(input: TurnInput[] | undefined): Array<Extract<TurnInput, { type: "local_image" | "image_url" }>> {
  return (input ?? []).filter(
    (part): part is Extract<TurnInput, { type: "local_image" | "image_url" }> =>
      part.type === "local_image" || part.type === "image_url"
  );
}

function imageKey(image: Extract<TurnInput, { type: "local_image" | "image_url" }>) {
  return image.type === "local_image" ? image.path : image.url;
}

function ReasoningBlock({ message }: { message: TranscriptMessage }) {
  const [expanded, setExpanded] = useState(message.timestamp === "now");
  const body = message.body.trim();

  return (
    <article className="w-full max-w-[780px]" aria-label="Reasoning message">
      <button
        type="button"
        className="group flex items-center gap-2 rounded-md py-1 text-left text-muted transition-colors hover:text-ink focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus"
        aria-expanded={expanded}
        onClick={() => setExpanded((value) => !value)}
      >
        <ChevronRight className={cn("h-4 w-4 transition-transform", expanded && "rotate-90")} />
        <span className="type-label-md">{message.title ?? "Reasoning"}</span>
        <span className="type-label-sm text-subtle">{message.timestamp}</span>
      </button>
      {expanded && body ? (
        <div className="type-body-md mt-2 border-l border-border pl-4 text-muted">
          <Markdown content={body} />
        </div>
      ) : null}
    </article>
  );
}

export function TranscriptSkeleton() {
  return (
    <div className="space-y-3" aria-label="Loading transcript">
      <Skeleton className="h-24" />
      <Skeleton className="h-32" />
      <Skeleton className="h-24" />
    </div>
  );
}
