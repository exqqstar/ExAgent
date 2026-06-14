import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import {
  Activity,
  ChevronRight,
  FileWarning,
  History,
  Pencil,
  Pin,
  PinOff,
  RefreshCw,
  ShieldAlert,
  ShieldCheck,
  Sparkles,
  Star,
  Trash2,
  XCircle
} from "lucide-react";
import { MemoryEntryEditor, type MemoryEntryEditorSubmit } from "@/components/memory/MemoryEntryEditor";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import {
  memoryAudit,
  memoryForget,
  memoryListCandidates,
  memoryPromote,
  memorySave,
  memorySearch,
  memoryUpdate,
  type MemoryAuditEventView,
  type MemoryEntryView,
  type MemoryHitView,
  type MemoryScope
} from "@/lib/api/memory";
import { cn } from "@/lib/utils";

type MemoryScopeMode = "project" | "global";
type EditorMode = "candidate_promote" | "edit" | "supersede";

interface EditorState {
  entry: MemoryEntryView | MemoryHitView;
  mode: EditorMode;
}

export function MemoryInspector({ projectId }: { projectId: string | null }) {
  const [scope, setScope] = useState<MemoryScopeMode>("project");
  const [candidates, setCandidates] = useState<MemoryEntryView[]>([]);
  const [activeEntries, setActiveEntries] = useState<MemoryHitView[]>([]);
  const [observations, setObservations] = useState<MemoryHitView[]>([]);
  const [auditEvents, setAuditEvents] = useState<MemoryAuditEventView[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [editor, setEditor] = useState<EditorState | null>(null);

  const loadMemory = useCallback(async () => {
    if (!projectId) {
      setCandidates([]);
      setActiveEntries([]);
      setObservations([]);
      setAuditEvents([]);
      return;
    }

    setLoading(true);
    setError(null);
    try {
      const [candidateResponse, activeResponse, observationResponse, auditResponse] = await Promise.all([
        memoryListCandidates(projectId, scope, "", 50),
        memorySearch(projectId, scope, "", false, 50),
        memorySearch(projectId, scope, "", true, 50),
        memoryAudit(projectId, scope, undefined, 50)
      ]);
      setCandidates(candidateResponse.candidates);
      setActiveEntries(activeResponse.hits.filter((hit) => hit.source !== "observation"));
      setObservations(observationResponse.hits.filter((hit) => hit.source === "observation"));
      setAuditEvents(auditResponse.events);
    } catch (loadError) {
      setError(loadError instanceof Error ? loadError.message : "Memory load failed");
    } finally {
      setLoading(false);
    }
  }, [projectId, scope]);

  useEffect(() => {
    void loadMemory();
  }, [loadMemory]);

  const regularCandidates = useMemo(() => candidates.filter((candidate) => !candidate.quarantined), [candidates]);
  const quarantinedCandidates = useMemo(() => candidates.filter((candidate) => candidate.quarantined), [candidates]);

  async function runAction(actionId: string, action: () => Promise<void>) {
    if (!projectId) {
      return;
    }

    setBusyAction(actionId);
    setError(null);
    try {
      await action();
      await loadMemory();
    } catch (actionError) {
      setError(actionError instanceof Error ? actionError.message : "Memory action failed");
    } finally {
      setBusyAction(null);
    }
  }

  async function promoteCandidate(candidate: MemoryEntryView, allowQuarantinedOverride = false) {
    await runAction(`promote:${candidate.id}`, async () => {
      await memoryPromote(projectId!, candidate.id, memoryScopeForItem(candidate), allowQuarantinedOverride);
    });
  }

  async function rejectCandidate(candidate: MemoryEntryView) {
    await runAction(`reject:${candidate.id}`, async () => {
      await memoryUpdate(projectId!, candidate.id, "reject", memoryScopeForItem(candidate));
    });
  }

  async function pinEntry(entry: MemoryHitView) {
    const pinned = !Boolean(entry.pinned);
    await runAction(`pin:${entry.id}`, async () => {
      await memoryUpdate(projectId!, entry.id, pinned ? "pin" : "unpin", memoryScopeForItem(entry));
    });
  }

  async function deleteEntry(entry: MemoryHitView) {
    await runAction(`delete:${entry.id}`, async () => {
      await memoryForget(projectId!, entry.id, memoryScopeForItem(entry));
    });
  }

  async function saveEditor(input: MemoryEntryEditorSubmit) {
    if (!projectId || !editor) {
      return;
    }

    const entryId = editor.entry.id;
    await runAction(`edit:${entryId}`, async () => {
      const entryScope = memoryScopeForItem(editor.entry);
      if (editor.mode === "candidate_promote") {
        await memorySave(projectId, entryScope, {
          kind: input.kind,
          title: input.title,
          content: input.content,
          files: input.files,
          concepts: input.concepts,
          source_observation_ids: "source_observation_ids" in editor.entry ? editor.entry.source_observation_ids : [],
          pinned: input.pinned
        });
        await memoryUpdate(projectId, entryId, "reject", entryScope);
      } else {
        await memoryUpdate(
          projectId,
          entryId,
          input.action,
          entryScope,
          input.kind,
          input.title,
          input.content,
          input.files,
          input.concepts,
          input.pinned,
          sourceObservationIdsForItem(editor.entry)
        );
      }
      setEditor(null);
    });
  }

  return (
    <TooltipProvider delayDuration={250}>
      <div className="flex h-full min-h-0 flex-col">
      <div className="border-b border-border px-4 py-3">
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0">
            <h2 className="type-title-lg truncate text-ink">Memory</h2>
            <p className="type-body-sm truncate text-muted">{projectId ? projectId : "No project selected"}</p>
          </div>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button type="button" variant="ghost" size="icon" aria-label="Refresh memory" onClick={() => void loadMemory()} disabled={loading}>
                <RefreshCw className={cn("h-4 w-4", loading && "animate-spin")} />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Refresh</TooltipContent>
          </Tooltip>
        </div>

        <div className="mt-3 flex items-center gap-2">
          <Button
            type="button"
            size="sm"
            variant={scope === "project" ? "secondary" : "ghost"}
            onClick={() => setScope("project")}
          >
            Project
          </Button>
          <Button
            type="button"
            size="sm"
            variant={scope === "global" ? "secondary" : "ghost"}
            onClick={() => setScope("global")}
          >
            Global
          </Button>
          <Badge variant="neutral">Global: human-promote-only</Badge>
        </div>
      </div>

      <ScrollArea className="min-h-0 flex-1">
        <div className="inspector-sections p-3">
          {error ? (
            <div className="mb-2 rounded-md border border-danger/30 bg-danger/10 px-3 py-2">
              <p className="type-body-md text-danger">{error}</p>
            </div>
          ) : null}

          {!projectId ? (
            <p className="type-body-md p-3 text-muted">No project selected.</p>
          ) : (
            <>
              <MemorySection
                defaultOpen
                icon={ShieldAlert}
                title="Pending"
                summary={`${candidates.length}`}
                accessory={candidates.length > 0 ? <Badge variant="warning">{candidates.length}</Badge> : null}
              >
                <div className="space-y-2">
                  {regularCandidates.length === 0 && quarantinedCandidates.length === 0 ? (
                    <EmptyText>No pending candidates.</EmptyText>
                  ) : null}
                  {regularCandidates.map((candidate) => (
                    <CandidateRow
                      key={candidate.id}
                      candidate={candidate}
                      busyAction={busyAction}
                      onPromote={promoteCandidate}
                      onEditPromote={(nextEntry) => setEditor({ entry: nextEntry, mode: "candidate_promote" })}
                      onReject={rejectCandidate}
                    />
                  ))}
                  {quarantinedCandidates.length > 0 ? (
                    <div className="space-y-2 border-l border-danger/50 pl-2">
                      <div className="flex items-center gap-2">
                        <Badge variant="danger">Quarantined</Badge>
                        <span className="type-label-sm text-danger">{quarantinedCandidates.length}</span>
                      </div>
                      {quarantinedCandidates.map((candidate) => (
                        <CandidateRow
                          key={candidate.id}
                          candidate={candidate}
                          busyAction={busyAction}
                          quarantinedGroup
                          onPromote={(entry) => promoteCandidate(entry, true)}
                          onEditPromote={(nextEntry) => setEditor({ entry: nextEntry, mode: "candidate_promote" })}
                          onReject={rejectCandidate}
                        />
                      ))}
                    </div>
                  ) : null}
                </div>
              </MemorySection>

              <MemorySection defaultOpen icon={ShieldCheck} title="Active" summary={`${activeEntries.length}`}>
                <div className="space-y-2">
                  {activeEntries.length === 0 ? <EmptyText>No active memories.</EmptyText> : null}
                  {activeEntries.map((entry) => (
                    <ActiveRow
                      key={entry.id}
                      entry={entry}
                      busyAction={busyAction}
                      onEdit={(nextEntry) => setEditor({ entry: nextEntry, mode: "edit" })}
                      onSupersede={(nextEntry) => setEditor({ entry: nextEntry, mode: "supersede" })}
                      onPin={pinEntry}
                      onDelete={deleteEntry}
                    />
                  ))}
                </div>
              </MemorySection>

              <MemorySection icon={FileWarning} title="Observations" summary={`${observations.length}`}>
                <div className="space-y-2">
                  {observations.length === 0 ? <EmptyText>No debug observations.</EmptyText> : null}
                  {observations.map((observation) => (
                    <ObservationRow key={observation.id} observation={observation} />
                  ))}
                </div>
              </MemorySection>

              <MemorySection defaultOpen icon={History} title="Activity" summary={`${auditEvents.length}`}>
                <div className="space-y-2">
                  {auditEvents.length === 0 ? <EmptyText>No memory activity.</EmptyText> : null}
                  {auditEvents.map((event) => (
                    <ActivityRow key={event.id} event={event} />
                  ))}
                </div>
              </MemorySection>
            </>
          )}
        </div>
      </ScrollArea>

      <MemoryEntryEditor
        entry={editor?.entry ?? null}
        mode={editor?.mode ?? "supersede"}
        open={editor !== null}
        saving={busyAction?.startsWith("edit:")}
        onOpenChange={(open) => {
          if (!open) {
            setEditor(null);
          }
        }}
        onSubmit={saveEditor}
      />
      </div>
    </TooltipProvider>
  );
}

function CandidateRow({
  candidate,
  busyAction,
  quarantinedGroup = false,
  onPromote,
  onEditPromote,
  onReject
}: {
  candidate: MemoryEntryView;
  busyAction: string | null;
  quarantinedGroup?: boolean;
  onPromote: (candidate: MemoryEntryView) => Promise<void>;
  onEditPromote: (candidate: MemoryEntryView) => void;
  onReject: (candidate: MemoryEntryView) => Promise<void>;
}) {
  return (
    <article
      data-testid={`memory-candidate-${candidate.id}`}
      className={cn(
        "rounded-md border px-2.5 py-2",
        candidate.quarantined || quarantinedGroup ? "border-danger/35 bg-danger/8" : "border-border bg-surface-2/65"
      )}
    >
      <MemoryRowHeader title={candidate.title} kind={candidate.kind} confidence={candidate.confidence}>
        <TrustBadges item={candidate} candidate />
      </MemoryRowHeader>
      <p className="type-body-sm mt-1 break-words text-muted">{candidate.body}</p>
      <FileRefs files={candidate.files} stale={candidate.stale} />
      {candidate.source_observation_ids.length > 0 ? (
        <details className="mt-2">
          <summary className="type-label-sm cursor-pointer text-muted">Source observations</summary>
          <ul className="mt-1 space-y-1">
            {candidate.source_observation_ids.map((observation, index) => (
              <li key={`${candidate.id}-source-${index}`} className="type-body-sm break-words border-l border-border pl-2 text-muted">
                {observation}
              </li>
            ))}
          </ul>
        </details>
      ) : null}
      {candidate.quarantined ? (
        <p className="type-body-sm mt-1 text-danger">
          {candidate.quarantine_reason ?? "Prompt-injection or sensitive provenance flagged"}
        </p>
      ) : null}
      <div className="mt-2 flex flex-wrap justify-end gap-1.5">
        <Button
          type="button"
          size="sm"
          variant="secondary"
          aria-label={`Promote ${candidate.title}`}
          disabled={busyAction !== null}
          onClick={() => void onPromote(candidate)}
        >
          <Sparkles className="h-3.5 w-3.5" />
          Promote
        </Button>
        <Button
          type="button"
          size="sm"
          variant="outline"
          aria-label={`Edit and promote ${candidate.title}`}
          disabled={busyAction !== null}
          onClick={() => onEditPromote(candidate)}
        >
          <Pencil className="h-3.5 w-3.5" />
          Edit&Promote
        </Button>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          aria-label={`Reject ${candidate.title}`}
          disabled={busyAction !== null}
          onClick={() => void onReject(candidate)}
        >
          <XCircle className="h-3.5 w-3.5" />
          Reject
        </Button>
      </div>
    </article>
  );
}

function ActiveRow({
  entry,
  busyAction,
  onEdit,
  onSupersede,
  onPin,
  onDelete
}: {
  entry: MemoryHitView;
  busyAction: string | null;
  onEdit: (entry: MemoryHitView) => void;
  onSupersede: (entry: MemoryHitView) => void;
  onPin: (entry: MemoryHitView) => Promise<void>;
  onDelete: (entry: MemoryHitView) => Promise<void>;
}) {
  const pinned = Boolean(entry.pinned);

  return (
    <article data-testid={`memory-active-entry-${entry.id}`} className="rounded-md border border-border bg-surface-2/65 px-2.5 py-2">
      <MemoryRowHeader title={entry.title} kind={entry.kind} confidence={entry.confidence}>
        {pinned ? (
          <Badge variant="primary" className="gap-1">
            <Star className="h-3 w-3 fill-current" />
            Pinned
          </Badge>
        ) : null}
        <TrustBadges item={entry} />
      </MemoryRowHeader>
      <p className="type-body-sm mt-1 break-words text-muted">{entry.body}</p>
      <div className="mt-1 flex flex-wrap items-center gap-1.5">
        <Badge variant="neutral">use_count {entry.use_count ?? 0}</Badge>
        <Badge variant={entry.scope === "global" ? "warning" : "neutral"}>{entry.scope}</Badge>
      </div>
      <FileRefs files={entry.files} stale={entry.stale} />
      {entry.supersedes_id ? (
        <p className="type-body-sm mt-1 text-muted">
          Version history <span className="type-code-sm">{entry.supersedes_id}</span>
        </p>
      ) : null}
      <div className="mt-2 flex flex-wrap justify-end gap-1.5">
        <Button
          type="button"
          size="sm"
          variant="ghost"
          aria-label={`Edit ${entry.title}`}
          disabled={busyAction !== null}
          onClick={() => onEdit(entry)}
        >
          <Pencil className="h-3.5 w-3.5" />
          Edit
        </Button>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          aria-label={`${pinned ? "Unpin" : "Pin"} ${entry.title}`}
          disabled={busyAction !== null}
          onClick={() => void onPin(entry)}
        >
          {pinned ? <PinOff className="h-3.5 w-3.5" /> : <Pin className="h-3.5 w-3.5" />}
          {pinned ? "Unpin" : "Pin"}
        </Button>
        <Button
          type="button"
          size="sm"
          variant="outline"
          aria-label={`Supersede ${entry.title}`}
          disabled={busyAction !== null}
          onClick={() => onSupersede(entry)}
        >
          <History className="h-3.5 w-3.5" />
          Supersede
        </Button>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          aria-label={`Delete ${entry.title}`}
          disabled={busyAction !== null}
          onClick={() => void onDelete(entry)}
        >
          <Trash2 className="h-3.5 w-3.5" />
          Delete
        </Button>
      </div>
    </article>
  );
}

function ObservationRow({ observation }: { observation: MemoryHitView }) {
  return (
    <article
      data-testid={`memory-observation-${observation.id}`}
      className="rounded-md border border-warning/30 bg-warning/8 px-2.5 py-2"
    >
      <MemoryRowHeader title={observation.title} kind={observation.kind} confidence={observation.confidence}>
        <Badge variant="warning">Observation</Badge>
        <Badge variant="warning">Low confidence</Badge>
        <TrustBadges item={observation} />
      </MemoryRowHeader>
      <p className="type-body-sm mt-1 break-words text-muted">{observation.body}</p>
      <FileRefs files={observation.files} stale={observation.stale} />
    </article>
  );
}

function ActivityRow({ event }: { event: MemoryAuditEventView }) {
  return (
    <div className="border-l border-border pl-3">
      <div className="flex min-w-0 items-center justify-between gap-2">
        <span className="type-label-md min-w-0 truncate text-ink">{event.action}</span>
        <Badge variant="neutral">{formatTime(event.created_at_ms)}</Badge>
      </div>
      <p className="type-body-sm mt-0.5 break-words text-muted">
        {event.actor} · {formatDetails(event.details)}
      </p>
    </div>
  );
}

function MemoryRowHeader({
  title,
  kind,
  confidence,
  children
}: {
  title: string;
  kind: string;
  confidence: number;
  children?: ReactNode;
}) {
  return (
    <div className="flex min-w-0 flex-wrap items-start gap-1.5">
      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 items-center gap-1.5">
          <Badge variant="neutral">{kind}</Badge>
          <h3 className="type-label-md min-w-0 truncate text-ink">{title}</h3>
        </div>
      </div>
      <div className="flex shrink-0 flex-wrap justify-end gap-1">
        <Badge variant={confidence < 0.5 ? "warning" : "info"}>{formatConfidence(confidence)}</Badge>
        {children}
      </div>
    </div>
  );
}

function TrustBadges({ item, candidate = false }: { item: MemoryEntryView | MemoryHitView; candidate?: boolean }) {
  return (
    <>
      {candidate ? <Badge variant="neutral">Candidate</Badge> : null}
      {item.stale ? <Badge variant="warning">Stale file reference</Badge> : null}
      {item.quarantined ? <Badge variant="danger">Quarantined</Badge> : null}
    </>
  );
}

function FileRefs({ files, stale }: { files: MemoryHitView["files"]; stale: boolean }) {
  if (files.length === 0) {
    return null;
  }

  return (
    <div className="mt-2 flex flex-wrap gap-1">
      {files.map((file, index) => (
        <Badge key={`${filePath(file)}-${index}`} variant={fileStale(file) || stale ? "warning" : "neutral"} className="max-w-full">
          <span className="type-code-sm truncate">{filePath(file)}</span>
        </Badge>
      ))}
    </div>
  );
}

function MemorySection({
  icon: Icon,
  title,
  summary,
  accessory,
  defaultOpen = false,
  children
}: {
  icon: typeof Activity;
  title: string;
  summary?: string;
  accessory?: ReactNode;
  defaultOpen?: boolean;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);

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
          {summary ? <span className="type-body-sm text-muted">{summary}</span> : null}
        </button>
        {accessory ? <div className="flex shrink-0 items-center">{accessory}</div> : null}
      </div>
      <div
        data-memory-section-content
        aria-hidden={!open}
        className={cn(
          "grid min-w-0 transition-[grid-template-rows,opacity] duration-200 ease-out",
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

function EmptyText({ children }: { children: ReactNode }) {
  return <p className="type-body-md text-muted">{children}</p>;
}

function formatConfidence(value: number) {
  return value.toFixed(2);
}

function formatTime(value: number) {
  if (!Number.isFinite(value) || value <= 0) {
    return "unknown";
  }
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit"
  }).format(new Date(value));
}

function formatDetails(details: unknown) {
  if (typeof details === "string") {
    return details;
  }
  if (details == null) {
    return "";
  }
  try {
    return JSON.stringify(details);
  } catch {
    return String(details);
  }
}

function filePath(file: string | { path: string }) {
  return typeof file === "string" ? file : file.path;
}

function fileStale(file: string | { stale?: boolean }) {
  return typeof file === "string" ? false : Boolean(file.stale);
}

function memoryScopeForItem(item: { scope: string }): MemoryScope {
  return item.scope === "global" ? "global" : "project";
}

function sourceObservationIdsForItem(item: { source_observation_ids?: string[] }): string[] {
  return item.source_observation_ids ?? [];
}
