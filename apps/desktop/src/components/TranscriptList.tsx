import {
  CheckCircle2,
  ChevronRight,
  CircleAlert,
  Clock3,
  FileText,
  GitBranchPlus,
  Info,
  Terminal
} from "lucide-react";
import { useState, type ReactNode } from "react";
import { ApprovalCard } from "@/components/ApprovalCard";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { TranscriptMessage, TurnInput } from "@/types";
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
  completed: "Completed",
  failed: "Failed",
  cancelled: "Cancelled"
};

export function TranscriptList({
  messages,
  loading = false,
  emptyLabel = "No transcript yet.",
  className,
  forkDisabled = false,
  readOnly = false,
  onForkFromTurn
}: {
  messages: TranscriptMessage[];
  loading?: boolean;
  emptyLabel?: string;
  className?: string;
  forkDisabled?: boolean;
  readOnly?: boolean;
  onForkFromTurn?: (threadId: string, turnId: string) => void;
}) {
  const { t } = useI18n();

  if (loading) {
    return <TranscriptSkeleton />;
  }

  if (messages.length === 0) {
    return <p className="type-body-md text-muted">{emptyLabel}</p>;
  }

  return (
    <div className={cn("flex flex-col gap-5", className)}>
      {messages.map((message) => (
        <TranscriptItem
          key={message.id}
          message={message}
          forkDisabled={forkDisabled}
          forkLabel={t("transcript.actions.forkFromHere")}
          readOnly={readOnly}
          onForkFromTurn={onForkFromTurn}
        />
      ))}
    </div>
  );
}

export function TranscriptItem({
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
  if (message.role === "approval") {
    return <ApprovalCard message={message} readOnly={readOnly} />;
  }

  if (message.role === "user") {
    const images = imageInputs(message.input);
    const hasBody = message.body.trim().length > 0;
    const canFork = Boolean(
      onForkFromTurn && message.threadId && message.turnId && message.turnStatus === "completed"
    );
    return (
      <article className="group flex items-start justify-end gap-2" aria-label="User message">
        {canFork && message.threadId && message.turnId ? (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                disabled={forkDisabled}
                aria-label={forkLabel}
                className="mt-1 h-7 w-7 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100"
                onClick={() => onForkFromTurn?.(message.threadId as string, message.turnId as string)}
              >
                <GitBranchPlus className="h-3.5 w-3.5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>{forkLabel}</TooltipContent>
          </Tooltip>
        ) : null}
        <div className="user-bubble max-w-[min(74%,680px)] rounded-lg px-4 py-2.5 text-ink sm:max-w-[min(68%,680px)]">
          {hasBody ? <p className="type-body-lg whitespace-pre-wrap break-words">{message.body}</p> : null}
          {images.length > 0 ? <UserImageGrid images={images} hasBody={hasBody} /> : null}
        </div>
      </article>
    );
  }

  if (message.role === "assistant") {
    return (
      <article className="w-full max-w-[780px] py-1" aria-label="Assistant message">
        <div className="type-body-lg break-words text-muted">
          <AssistantText text={message.body} />
        </div>
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

function GoalReportCard({ message }: { message: TranscriptMessage }) {
  const report = message.goalReport;
  if (!report) {
    return null;
  }
  const approvalsLabel = `${report.pending_approvals_count} ${report.pending_approvals_count === 1 ? "approval" : "approvals"} waiting in Inbox`;

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
      {report.changed_files.length > 0 ? (
        <div className="mt-3">
          <div className="flex items-center gap-2 text-muted">
            <FileText className="h-4 w-4" />
            <span className="type-label-sm">Changed files</span>
          </div>
          <ul className="mt-2 space-y-1">
            {report.changed_files.map((file) => (
              <li key={file} className="type-code-sm break-all rounded border border-border bg-surface-2 px-2 py-1 text-muted">
                {file}
              </li>
            ))}
          </ul>
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
          <AssistantText text={body} />
        </div>
      ) : null}
    </article>
  );
}

function AssistantText({ text }: { text: string }) {
  const blocks = text.split(/\n{2,}/);
  return (
    <div className="space-y-4">
      {blocks.map((block, index) => (
        <AssistantBlock key={`${index}-${block.slice(0, 12)}`} block={block} />
      ))}
    </div>
  );
}

function AssistantBlock({ block }: { block: string }) {
  const trimmed = block.trim();
  if (!trimmed) {
    return null;
  }

  if (trimmed.startsWith("```")) {
    const code = trimmed
      .replace(/^```[^\n]*\n?/, "")
      .replace(/\n?```$/, "");
    return (
      <pre className="type-code-sm message-card overflow-auto rounded-lg border border-border px-3 py-2 text-muted">
        {code}
      </pre>
    );
  }

  const lines = trimmed.split("\n");
  const isList = lines.every((line) => /^[-*]\s+/.test(line.trim()));
  if (isList) {
    return (
      <ul className="space-y-1 pl-5">
        {lines.map((line, index) => (
          <li key={`${index}-${line}`} className="list-disc">
            {renderInlineMarkdown(line.trim().replace(/^[-*]\s+/, ""))}
          </li>
        ))}
      </ul>
    );
  }

  return (
    <p className="whitespace-pre-wrap leading-relaxed">
      {renderInlineMarkdown(trimmed)}
    </p>
  );
}

function renderInlineMarkdown(text: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  const pattern = /(\*\*[^*]+\*\*|`[^`]+`)/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(text)) !== null) {
    if (match.index > lastIndex) {
      nodes.push(text.slice(lastIndex, match.index));
    }
    const token = match[0];
    if (token.startsWith("**")) {
      nodes.push(
        <strong key={`${match.index}-bold`} className="font-semibold text-ink">
          {token.slice(2, -2)}
        </strong>
      );
    } else {
      nodes.push(
        <code key={`${match.index}-code`} className="rounded bg-surface-2 px-1 py-0.5 font-mono text-[0.92em] text-ink">
          {token.slice(1, -1)}
        </code>
      );
    }
    lastIndex = match.index + token.length;
  }

  if (lastIndex < text.length) {
    nodes.push(text.slice(lastIndex));
  }
  return nodes;
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
