import { CheckCircle2, ChevronRight, CircleAlert, Info, Terminal } from "lucide-react";
import { useState, type ReactNode } from "react";
import { ApprovalCard } from "@/components/ApprovalCard";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import type { TranscriptMessage, TurnInput } from "@/types";
import { fileBaseName, localFileAssetSrc } from "@/lib/media";
import { cn } from "@/lib/utils";

const roleLabel: Record<TranscriptMessage["role"], string> = {
  user: "You",
  assistant: "ExAgent",
  reasoning: "Reasoning",
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
  className
}: {
  messages: TranscriptMessage[];
  loading?: boolean;
  emptyLabel?: string;
  className?: string;
}) {
  if (loading) {
    return <TranscriptSkeleton />;
  }

  if (messages.length === 0) {
    return <p className="type-body-md text-muted">{emptyLabel}</p>;
  }

  return (
    <div className={cn("flex flex-col gap-5", className)}>
      {messages.map((message) => (
        <TranscriptItem key={message.id} message={message} />
      ))}
    </div>
  );
}

export function TranscriptItem({ message }: { message: TranscriptMessage }) {
  if (message.role === "approval") {
    return <ApprovalCard message={message} />;
  }

  if (message.role === "user") {
    const images = imageInputs(message.input);
    const hasBody = message.body.trim().length > 0;
    return (
      <article className="flex justify-end" aria-label="User message">
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
