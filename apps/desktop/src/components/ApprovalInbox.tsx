import { useMemo, useState } from "react";
import { Check, ChevronDown, ChevronRight, RotateCcw, X } from "lucide-react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useI18n, type TranslationKey } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import { useWorkbenchStore } from "@/stores/workbenchStore";
import type { ApprovalActionStatus, PendingApprovalItem } from "@/types";

type ApprovalGroup = {
  key: string;
  label: string;
  items: PendingApprovalItem[];
};

export function ApprovalInbox() {
  const { t } = useI18n();
  const workbench = useWorkbenchStore();
  const [expandedIds, setExpandedIds] = useState<Set<string>>(new Set());
  const [rollbackItem, setRollbackItem] = useState<PendingApprovalItem | null>(null);
  const [rollbackConfirmed, setRollbackConfirmed] = useState(false);
  const selectedCount = workbench.selectedApprovalIds.size;
  const groups = useMemo(() => groupApprovals(workbench.pendingApprovals, t), [workbench.pendingApprovals, t]);

  function toggleExpanded(approvalId: string) {
    setExpandedIds((current) => {
      const next = new Set(current);
      if (next.has(approvalId)) {
        next.delete(approvalId);
      } else {
        next.add(approvalId);
      }
      return next;
    });
  }

  function openRollback(item: PendingApprovalItem) {
    setRollbackItem(item);
    setRollbackConfirmed(false);
  }

  async function confirmRollback() {
    if (!rollbackItem || !rollbackConfirmed) {
      return;
    }
    const item = rollbackItem;
    setRollbackItem(null);
    setRollbackConfirmed(false);
    await workbench.rejectAndRollbackApproval(item);
  }

  return (
    <>
      <div className="flex h-full min-h-0 flex-col">
        <div className="shrink-0 border-b border-border px-4 py-3">
          <div className="flex items-center justify-between gap-3">
            <div className="min-w-0">
              <h2 className="type-title-lg text-ink">{t("approvals.inbox.title")}</h2>
              <p className="type-body-sm mt-1 text-muted">
                {workbench.pendingApprovals.length} {t("approvals.inbox.pending")}
              </p>
            </div>
            {selectedCount > 0 ? (
              <div className="flex shrink-0 items-center gap-2">
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={workbench.clearApprovalSelection}
                >
                  {t("approvals.inbox.clearSelection")}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  aria-label={t("approvals.inbox.approveSelectedAria")}
                  disabled={workbench.approvalsStatus === "submitting"}
                  onClick={() => void workbench.approveSelectedApprovals()}
                >
                  <Check className="h-3.5 w-3.5" />
                  {t("approvals.inbox.approveSelected")}
                </Button>
              </div>
            ) : null}
          </div>
          {workbench.approvalsStatus === "loading" ? (
            <p className="type-body-sm mt-2 text-muted">{t("approvals.inbox.loading")}</p>
          ) : null}
          {workbench.approvalActionStatus ? (
            <p className="type-body-sm mt-2 text-muted">
              {formatApprovalActionStatus(workbench.approvalActionStatus, t)}
            </p>
          ) : null}
          {workbench.approvalsError ? (
            <p className="type-body-sm mt-2 text-danger">{workbench.approvalsError}</p>
          ) : null}
        </div>

        <ScrollArea className="min-h-0 flex-1">
          {groups.length === 0 ? (
            <div className="px-4 py-6">
              <p className="type-body-md text-muted">{t("approvals.inbox.empty")}</p>
            </div>
          ) : (
            <div className="space-y-4 px-4 py-4">
              {groups.map((group) => (
                <section key={group.key} aria-label={group.label} className="space-y-2">
                  <div className="flex items-center justify-between gap-2">
                    <h3 className="type-label-md min-w-0 truncate text-muted">{group.label}</h3>
                    <Badge variant="neutral">
                      {group.items.length} {t("approvals.inbox.pending")}
                    </Badge>
                  </div>
                  <div className="space-y-2">
                    {group.items.map((item) => {
                      const expanded = expandedIds.has(item.approval_id);
                      const selected = workbench.selectedApprovalIds.has(item.approval_id);
                      return (
                        <article
                          key={item.approval_id}
                          className="rounded-lg border border-border bg-surface px-3 py-3"
                        >
                          <div className="flex min-w-0 items-start gap-3">
                            <label className="mt-1 flex h-5 w-5 shrink-0 items-center justify-center">
                              <span className="sr-only">
                                {t("approvals.inbox.select")} {item.summary}
                              </span>
                              <input
                                type="checkbox"
                                className="h-4 w-4 rounded border-border bg-surface-2"
                                checked={selected}
                                onChange={() => workbench.toggleApprovalSelection(item.approval_id)}
                              />
                            </label>
                            <div className="min-w-0 flex-1">
                              <div className="flex min-w-0 items-start justify-between gap-2">
                                <div className="min-w-0">
                                  <p className="type-title-sm truncate text-ink">{item.summary}</p>
                                  <div className="mt-1 flex flex-wrap items-center gap-1.5">
                                    <Badge variant="warning">{item.kind}</Badge>
                                    <Badge variant={item.checkpoint_id ? "info" : "neutral"}>
                                      {item.checkpoint_id
                                        ? `${t("approvals.inbox.checkpoint")} ${item.checkpoint_id}`
                                        : t("approvals.inbox.rollbackUnavailable")}
                                    </Badge>
                                    <span className="type-code-sm text-subtle">
                                      {t("approvals.inbox.requested")}{" "}
                                      {formatRequestedAt(item.requested_at_ms, t("approvals.inbox.requestedUnknown"))}
                                    </span>
                                  </div>
                                </div>
                                <Button
                                  type="button"
                                  size="sm"
                                  variant="ghost"
                                  aria-label={formatTemplate(
                                    expanded ? t("approvals.inbox.hideDetailsFor") : t("approvals.inbox.detailsFor"),
                                    { summary: item.summary }
                                  )}
                                  onClick={() => toggleExpanded(item.approval_id)}
                                >
                                  {expanded ? (
                                    <ChevronDown className="h-3.5 w-3.5" />
                                  ) : (
                                    <ChevronRight className="h-3.5 w-3.5" />
                                  )}
                                  {expanded ? t("approvals.inbox.hideDetails") : t("approvals.inbox.details")}
                                </Button>
                              </div>
                              {expanded ? (
                                <pre className="type-code-sm mt-3 max-h-56 overflow-auto rounded-md border border-border bg-surface-2 p-3 text-muted">
                                  {item.detail}
                                </pre>
                              ) : null}
                              <div className="mt-3 flex flex-wrap items-center gap-2">
                                <Button
                                  type="button"
                                  size="sm"
                                  aria-label={formatTemplate(t("approvals.inbox.approveFor"), {
                                    summary: item.summary
                                  })}
                                  disabled={workbench.approvalsStatus === "submitting"}
                                  onClick={() => void workbench.approveInboxApproval(item)}
                                >
                                  <Check className="h-3.5 w-3.5" />
                                  {t("approvals.inbox.approve")}
                                </Button>
                                <Button
                                  type="button"
                                  size="sm"
                                  variant="danger"
                                  aria-label={formatTemplate(t("approvals.inbox.rejectFor"), {
                                    summary: item.summary
                                  })}
                                  disabled={workbench.approvalsStatus === "submitting"}
                                  onClick={() => void workbench.rejectInboxApproval(item)}
                                >
                                  <X className="h-3.5 w-3.5" />
                                  {t("approvals.inbox.reject")}
                                </Button>
                                {item.checkpoint_id ? (
                                  <Button
                                    type="button"
                                    size="sm"
                                    variant="outline"
                                    aria-label={formatTemplate(t("approvals.inbox.rejectRollbackFor"), {
                                      summary: item.summary
                                    })}
                                    disabled={workbench.approvalsStatus === "submitting"}
                                    onClick={() => openRollback(item)}
                                  >
                                    <RotateCcw className="h-3.5 w-3.5" />
                                    {t("approvals.inbox.rejectRollback")}
                                  </Button>
                                ) : (
                                  <span className="type-body-sm text-muted">
                                    {t("approvals.inbox.rollbackUnavailable")}
                                  </span>
                                )}
                              </div>
                            </div>
                          </div>
                        </article>
                      );
                    })}
                  </div>
                </section>
              ))}
            </div>
          )}
        </ScrollArea>
      </div>

      <AlertDialog
        open={rollbackItem !== null}
        onOpenChange={(open) => {
          if (!open) {
            setRollbackItem(null);
            setRollbackConfirmed(false);
          }
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("approvals.inbox.confirmTitle")}</AlertDialogTitle>
            <AlertDialogDescription>{t("approvals.inbox.confirmDescription")}</AlertDialogDescription>
          </AlertDialogHeader>
          {rollbackItem ? (
            <div className="space-y-3">
              <div className="rounded-md border border-border bg-surface-2 p-3">
                <p className="type-title-sm text-ink">{rollbackItem.summary}</p>
                <p className="type-code-sm mt-1 break-all text-muted">{rollbackItem.checkpoint_id}</p>
                <pre className="type-code-sm mt-3 max-h-40 overflow-auto rounded border border-border bg-surface p-2 text-muted">
                  {rollbackItem.detail}
                </pre>
              </div>
              <label className="flex items-start gap-2">
                <input
                  type="checkbox"
                  className={cn("mt-0.5 h-4 w-4 rounded border-border bg-surface-2")}
                  checked={rollbackConfirmed}
                  onChange={(event) => setRollbackConfirmed(event.target.checked)}
                />
                <span className="type-body-sm text-ink">{t("approvals.inbox.confirmUnderstanding")}</span>
              </label>
            </div>
          ) : null}
          <AlertDialogFooter>
            <AlertDialogCancel>{t("approvals.inbox.cancel")}</AlertDialogCancel>
            <AlertDialogAction disabled={!rollbackConfirmed} onClick={() => void confirmRollback()}>
              {t("approvals.inbox.confirmAction")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

function groupApprovals(
  approvals: PendingApprovalItem[],
  t: (key: "approvals.inbox.groupGoal" | "approvals.inbox.groupThread") => string
): ApprovalGroup[] {
  const groups = new Map<string, ApprovalGroup>();
  for (const item of approvals) {
    const key = item.goal_id ? `goal:${item.goal_id}` : `thread:${item.thread_id}`;
    const label = item.goal_id
      ? `${t("approvals.inbox.groupGoal")} ${item.goal_id}`
      : `${t("approvals.inbox.groupThread")} ${item.thread_id}`;
    const group = groups.get(key);
    if (group) {
      group.items.push(item);
    } else {
      groups.set(key, { key, label, items: [item] });
    }
  }
  return Array.from(groups.values());
}

function formatApprovalActionStatus(
  status: ApprovalActionStatus,
  t: (key: TranslationKey) => string
) {
  switch (status.type) {
    case "approval_decision":
      return formatTemplate(
        t(status.decision === "approved" ? "approvals.inbox.status.approved" : "approvals.inbox.status.denied"),
        { approvalId: status.approval_id }
      );
    case "batch_approved":
      return formatTemplate(t("approvals.inbox.status.batchApproved"), {
        count: status.count,
        approvalNoun:
          status.count === 1 ? t("approvals.inbox.approvalSingular") : t("approvals.inbox.approvalPlural")
      });
    case "batch_partial_failed":
      return formatTemplate(t("approvals.inbox.status.batchPartialFailed"), {
        completed: status.completed,
        total: status.total,
        approvalId: status.approval_id,
        error: status.error
      });
    case "rollback_unavailable":
      return formatTemplate(t("approvals.inbox.status.rollbackUnavailable"), {
        approvalId: status.approval_id
      });
    case "rollback_restored":
      return formatTemplate(t("approvals.inbox.status.rollbackRestored"), {
        approvalId: status.approval_id,
        checkpointId: status.checkpoint_id
      });
    case "rollback_failed_after_reject":
      return formatTemplate(t("approvals.inbox.status.rollbackFailedAfterReject"), {
        approvalId: status.approval_id,
        error: status.error
      });
  }
}

function formatTemplate(template: string, values: Record<string, string | number>) {
  return template.replace(/\{(\w+)\}/g, (match, key) => {
    const value = values[key];
    return value === undefined ? match : String(value);
  });
}

function formatRequestedAt(requestedAtMs: number, fallback: string) {
  if (!Number.isFinite(requestedAtMs) || requestedAtMs <= 0) {
    return fallback;
  }
  return new Date(requestedAtMs).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit"
  });
}
