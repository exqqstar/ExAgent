import { TokenUsagePanel } from "@/components/TokenUsagePanel";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useI18n } from "@/lib/i18n";
import type { BackendRuntimeEvent, ThreadTokenUsage, TokenUsageInfo } from "@/types";

export function EventDetail({
  event,
  allEvents
}: {
  event: BackendRuntimeEvent | null;
  allEvents: BackendRuntimeEvent[];
}) {
  const { t } = useI18n();
  void allEvents;

  if (!event) {
    return <p className="type-body-md text-muted">{t("eventLog.selectEvent")}</p>;
  }

  return (
    <article data-testid="event-detail" className="flex h-full min-h-0 w-full min-w-0 flex-col gap-4 overflow-hidden">
      <header
        data-testid="event-detail-header"
        className="grid w-full min-w-0 grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] items-start gap-4 pb-1"
      >
        <div data-testid="event-detail-meta" className="contents">
          <EventMeta label="event_id" value={event.event_id} />
          <EventMeta label="turn_id" value={event.turn_id ?? "No turn"} />
        </div>
        <Badge data-testid="event-detail-time" variant="neutral" className="shrink-0 justify-self-end">
          {t("eventLog.timeNotRecorded")}
        </Badge>
      </header>
      <ScrollArea data-testid="event-detail-scroll" className="min-h-0 flex-1 overflow-hidden">
        <div className="space-y-4 pr-3">{renderEventBody(event)}</div>
      </ScrollArea>
    </article>
  );
}

function renderEventBody(event: BackendRuntimeEvent) {
  switch (event.kind.type) {
    case "token_count":
      return <TokenUsagePanel usage={tokenUsageFromInfo(event.thread_id, event.kind.info)} />;
    case "tool_result":
      return (
        <div className="space-y-3">
          <div className="flex flex-wrap items-center gap-2">
            <Badge variant="neutral">{event.kind.result.tool_name}</Badge>
            <Badge variant={event.kind.result.status === "success" ? "success" : "warning"}>
              {event.kind.result.status}
            </Badge>
            <span className="type-code-sm text-muted">{event.kind.result.tool_call_id}</span>
          </div>
          <PreBlock label="Output" value={event.kind.result.content} defaultOpen />
          {event.kind.result.meta != null ? <JsonBlock label="Meta" value={event.kind.result.meta} /> : null}
          {event.kind.result.parts != null ? <JsonBlock label="Parts" value={event.kind.result.parts} /> : null}
        </div>
      );
    case "assistant_turn":
      return (
        <div className="space-y-3">
          <ProseBlock value={event.kind.turn.text ?? "Assistant turn"} />
          {event.kind.turn.tool_calls.length > 0 ? <JsonBlock label="Tool calls" value={event.kind.turn.tool_calls} /> : null}
        </div>
      );
    case "reasoning":
      return (
        <div className="space-y-3">
          {event.kind.summary?.length ? <ProseBlock title="Summary" value={event.kind.summary.join("\n\n")} /> : null}
          {event.kind.content?.length ? <ProseBlock title="Content" value={event.kind.content.join("\n\n")} /> : null}
          {!event.kind.summary?.length && !event.kind.content?.length ? <ProseBlock value="No reasoning content." /> : null}
        </div>
      );
    case "assistant_text_delta":
    case "reasoning_delta":
      return <PreBlock label="Delta" value={event.kind.delta} defaultOpen />;
    case "tool_invocation_output_delta":
      return (
        <div className="space-y-3">
          <FieldGrid
            fields={[
              ["invocation_id", event.kind.invocation_id],
              ["stream", event.kind.stream],
              ["sequence", String(event.kind.sequence)]
            ]}
          />
          <PreBlock label="Chunk" value={event.kind.chunk} defaultOpen />
        </div>
      );
    case "exec_output":
      return (
        <div className="space-y-3">
          <FieldGrid
            fields={[
              ["exec_session_id", event.kind.exec_session_id],
              ["stream", event.kind.stream],
              ["sequence", event.kind.sequence == null ? "not recorded" : String(event.kind.sequence)]
            ]}
          />
          <PreBlock label="Chunk" value={event.kind.chunk} defaultOpen />
        </div>
      );
    case "tool_invocation_started":
      return (
        <FieldGrid
          fields={[
            ["tool_name", event.kind.tool_name],
            ["tool_call_id", event.kind.tool_call_id],
            ["invocation_id", event.kind.invocation_id],
            ["mutating", String(event.kind.mutating)]
          ]}
        />
      );
    case "tool_invocation_waiting_approval":
      return (
        <FieldGrid
          fields={[
            ["approval_id", event.kind.approval_id],
            ["invocation_id", event.kind.invocation_id],
            ["reason", event.kind.reason]
          ]}
        />
      );
    case "tool_invocation_waiting_user_input":
      return (
        <FieldGrid
          fields={[
            ["request_id", event.kind.request_id],
            ["invocation_id", event.kind.invocation_id],
            ["reason", event.kind.reason]
          ]}
        />
      );
    case "tool_invocation_completed":
      return (
        <FieldGrid
          fields={[
            ["tool_name", event.kind.tool_name],
            ["tool_call_id", event.kind.tool_call_id],
            ["invocation_id", event.kind.invocation_id],
            ["status", event.kind.status]
          ]}
        />
      );
    case "tool_invocation_failed":
      return (
        <FieldGrid
          fields={[
            ["tool_name", event.kind.tool_name],
            ["tool_call_id", event.kind.tool_call_id],
            ["invocation_id", event.kind.invocation_id],
            ["message", event.kind.message]
          ]}
        />
      );
    case "tool_invocation_cancelled":
      return (
        <FieldGrid
          fields={[
            ["tool_name", event.kind.tool_name],
            ["tool_call_id", event.kind.tool_call_id],
            ["invocation_id", event.kind.invocation_id],
            ["reason", event.kind.reason]
          ]}
        />
      );
    case "approval_requested":
      return (
        <FieldGrid
          fields={[
            ["approval_id", event.kind.approval_id],
            ["tool_name", event.kind.tool_name],
            ["reason", event.kind.reason],
            ["checkpoint_id", event.kind.checkpoint_id ?? "none"]
          ]}
        />
      );
    case "approval_decision":
      return (
        <FieldGrid
          fields={[
            ["approval_id", event.kind.approval_id],
            ["status", event.kind.status],
            ["note", event.kind.note ?? "none"]
          ]}
        />
      );
    case "user_input_requested":
      return (
        <div className="space-y-3">
          <FieldGrid
            fields={[
              ["request_id", event.kind.request_id],
              ["tool_name", event.kind.tool_name]
            ]}
          />
          <JsonBlock label="Questions" value={event.kind.questions} />
        </div>
      );
    case "user_input_resolved":
      return (
        <FieldGrid
          fields={[
            ["request_id", event.kind.request_id],
            ["dismissed", String(event.kind.dismissed)]
          ]}
        />
      );
    case "compaction_written":
      return <ProseBlock title="Summary" value={event.kind.summary.summary} />;
    case "subagent_spawned":
      return (
        <FieldGrid
          fields={[
            ["task_name", event.kind.task_name],
            ["parent_thread_id", event.kind.parent_thread_id],
            ["child_thread_id", event.kind.child_thread_id],
            ["message_preview", event.kind.message_preview],
            ["tool_call_id", event.kind.tool_call_id],
            ["invocation_id", event.kind.invocation_id]
          ]}
        />
      );
    case "subagent_closed":
      return (
        <FieldGrid
          fields={[
            ["parent_thread_id", event.kind.parent_thread_id],
            ["closed_thread_id", event.kind.closed_thread_id],
            ["agent_path", event.kind.agent_path],
            ["tool_call_id", event.kind.tool_call_id],
            ["invocation_id", event.kind.invocation_id]
          ]}
        />
      );
    case "inter_agent_message_sent":
      return (
        <FieldGrid
          fields={[
            ["author_thread_id", event.kind.author_thread_id],
            ["recipient_thread_id", event.kind.recipient_thread_id],
            ["author_path", event.kind.author_path],
            ["recipient_path", event.kind.recipient_path],
            ["content_preview", event.kind.content_preview],
            ["followup", String(event.kind.followup)],
            ["started_turn_id", event.kind.started_turn_id ?? "none"]
          ]}
        />
      );
    case "thread_goal_updated":
      return <JsonBlock label="Goal" value={event.kind.goal} defaultOpen />;
    case "thread_goal_mode_updated":
      return (
        <FieldGrid
          fields={[
            ["thread_id", event.kind.thread_id],
            ["goal_id", event.kind.goal_id],
            ["mode", event.kind.mode]
          ]}
        />
      );
    case "thread_goal_cleared":
      return <FieldGrid fields={[["thread_id", event.kind.thread_id]]} />;
    case "thread_goal_continuation_started":
    case "thread_goal_turn_started":
      return <FieldGrid fields={[["goal_id", event.kind.goal_id]]} />;
    case "thread_goal_continuation_suppressed":
      return (
        <FieldGrid
          fields={[
            ["goal_id", event.kind.goal_id],
            ["reason", event.kind.reason]
          ]}
        />
      );
    case "thread_goal_tool_completed":
      return (
        <div className="space-y-3">
          <FieldGrid fields={[["goal_id", event.kind.goal_id]]} />
          {event.kind.changed_files?.length ? <JsonBlock label="Changed files" value={event.kind.changed_files} /> : null}
        </div>
      );
    case "thread_goal_report":
      return <JsonBlock label="Goal report" value={event.kind.report} defaultOpen />;
    case "review_submitted":
      return (
        <div className="space-y-3">
          <FieldGrid
            fields={[
              ["ticket_id", event.kind.ticket_id],
              ["goal_id", event.kind.goal_id],
              ["verdict", event.kind.verdict],
              ["reviewed_hash", event.kind.reviewed_hash ?? "none"],
              ["reject_category", event.kind.reject_category ?? "none"],
              ["checkpoint_id", event.kind.checkpoint_id ?? "none"]
            ]}
          />
          {event.kind.findings ? <ProseBlock title="Findings" value={event.kind.findings} /> : null}
        </div>
      );
    case "open_question_recorded":
      return (
        <FieldGrid
          fields={[
            ["question_id", event.kind.question_id],
            ["goal_id", event.kind.goal_id],
            ["question", event.kind.question],
            ["blocks_what", event.kind.blocks_what]
          ]}
        />
      );
    case "open_question_resolved":
      return (
        <FieldGrid
          fields={[
            ["question_id", event.kind.question_id],
            ["goal_id", event.kind.goal_id],
            ["answer", event.kind.answer ?? "none"]
          ]}
        />
      );
    case "runtime_error":
      return <ProseBlock title="Error" value={event.kind.message} />;
    default:
      return <JsonBlock label="Raw event kind" value={event.kind} defaultOpen />;
  }
}

function tokenUsageFromInfo(threadId: string, info: TokenUsageInfo | null | undefined): ThreadTokenUsage | null {
  if (!info) {
    return null;
  }
  return {
    threadId,
    total: info.total_token_usage,
    last: info.last_token_usage,
    modelContextWindow: info.model_context_window ?? null
  };
}

function EventMeta({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0 space-y-1">
      <div className="type-label-sm text-muted">{label}</div>
      <div className="type-code-sm min-w-0 truncate text-ink" title={value}>
        {value}
      </div>
    </div>
  );
}

function FieldGrid({ fields }: { fields: Array<[string, string]> }) {
  return (
    <dl className="grid gap-2">
      {fields.map(([label, value]) => (
        <div key={label} className="grid min-w-0 grid-cols-[128px_minmax(0,1fr)] gap-3 border-l border-border py-1 pl-3">
          <dt className="type-label-sm min-w-0 truncate text-muted">{label}</dt>
          <dd className="type-body-sm min-w-0 break-words text-ink">{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function ProseBlock({ title, value }: { title?: string; value: string }) {
  return (
    <section className="space-y-2">
      {title ? <h4 className="type-label-md text-ink">{title}</h4> : null}
      <p className="type-body-md whitespace-pre-wrap break-words text-ink">{value}</p>
    </section>
  );
}

function PreBlock({
  label,
  value,
  defaultOpen = false
}: {
  label: string;
  value: string;
  defaultOpen?: boolean;
}) {
  return (
    <details className="space-y-2 py-1" open={defaultOpen}>
      <summary className="type-label-md cursor-pointer text-ink marker:text-muted">{label}</summary>
      <pre className="type-code-sm whitespace-pre-wrap break-words border-l border-border py-1 pl-3 text-muted">
        {value}
      </pre>
    </details>
  );
}

function JsonBlock({
  label,
  value,
  defaultOpen = true
}: {
  label: string;
  value: unknown;
  defaultOpen?: boolean;
}) {
  return <PreBlock label={label} value={JSON.stringify(value, null, 2)} defaultOpen={defaultOpen} />;
}
