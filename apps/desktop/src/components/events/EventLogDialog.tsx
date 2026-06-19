import { useEffect, useMemo, useState } from "react";
import { exagentClient } from "@/api/exagentClient";
import { EventDetail } from "@/components/events/EventDetail";
import { EventList } from "@/components/events/EventList";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from "@/components/ui/dialog";
import { groupEventsByTurn, isDeltaEvent } from "@/lib/eventPresentation";
import { useI18n } from "@/lib/i18n";
import type { BackendRuntimeEvent } from "@/types";

export function EventLogDialog({
  projectId,
  threadId,
  open,
  onClose
}: {
  projectId: string | null;
  threadId: string | null;
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const [events, setEvents] = useState<BackendRuntimeEvent[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [showDeltas, setShowDeltas] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) {
      return;
    }

    if (!projectId || !threadId) {
      setEvents([]);
      setSelectedId(null);
      setLoading(false);
      setError(t("eventLog.noActiveThread"));
      return;
    }

    let cancelled = false;
    setLoading(true);
    setError(null);
    setEvents([]);
    setSelectedId(null);
    setShowDeltas(false);

    exagentClient.replayAllEvents(projectId, threadId)
      .then((loadedEvents) => {
        if (cancelled) {
          return;
        }
        setEvents(loadedEvents);
        setSelectedId(firstVisibleEventId(loadedEvents, false));
      })
      .catch((cause: unknown) => {
        if (cancelled) {
          return;
        }
        setError(errorMessage(cause));
      })
      .finally(() => {
        if (!cancelled) {
          setLoading(false);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [open, projectId, threadId, t]);

  const visibleEvents = useMemo(
    () => (showDeltas ? events : events.filter((event) => !isDeltaEvent(event.kind.type))),
    [events, showDeltas]
  );
  const selectedEvent = visibleEvents.find((event) => event.event_id === selectedId) ?? visibleEvents[0] ?? null;
  const groups = useMemo(() => groupEventsByTurn(visibleEvents), [visibleEvents]);

  return (
    <Dialog
      open={open}
      onOpenChange={(nextOpen) => {
        if (!nextOpen) {
          onClose();
        }
      }}
    >
      <DialogContent className="h-[min(760px,calc(100vh-40px))] w-[min(1120px,calc(100vw-32px))] max-w-none grid-rows-[auto_minmax(0,1fr)] gap-4 overflow-hidden p-4">
        <DialogHeader className="pr-8">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <DialogTitle>{t("eventLog.title").replace("{count}", String(events.length))}</DialogTitle>
            <label className="type-label-md flex items-center gap-2 text-muted">
              <input
                type="checkbox"
                className="h-4 w-4 accent-primary"
                checked={showDeltas}
                onChange={(event) => {
                  const nextShowDeltas = event.currentTarget.checked;
                  setShowDeltas(nextShowDeltas);
                  setSelectedId(firstVisibleEventId(events, nextShowDeltas));
                }}
              />
              {t("eventLog.showStreaming")}
            </label>
          </div>
          <DialogDescription>{t("eventLog.description")}</DialogDescription>
        </DialogHeader>

        {loading ? (
          <div className="type-body-md flex min-h-0 items-center justify-center text-muted">{t("eventLog.loading")}</div>
        ) : error ? (
          <div className="type-body-md rounded-md border border-danger/40 bg-danger/10 p-3 text-danger">{error}</div>
        ) : events.length === 0 ? (
          <div className="type-body-md flex min-h-0 items-center justify-center text-muted">{t("eventLog.empty")}</div>
        ) : visibleEvents.length === 0 ? (
          <div className="type-body-md flex min-h-0 items-center justify-center text-muted">
            {t("eventLog.onlyStreamingHidden")}
          </div>
        ) : (
          <div data-testid="event-log-layout" className="grid h-full min-h-0 overflow-hidden gap-4 md:grid-cols-[280px_minmax(0,1fr)]">
            <aside className="flex min-h-0 overflow-hidden rounded-md border border-border bg-surface-1 p-2">
              <EventList groups={groups} selectedId={selectedEvent?.event_id ?? null} onSelect={setSelectedId} />
            </aside>
            <section data-testid="event-log-detail" className="flex min-h-0 min-w-0 overflow-hidden">
              <EventDetail event={selectedEvent} allEvents={events} />
            </section>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

function firstVisibleEventId(events: BackendRuntimeEvent[], showDeltas: boolean) {
  return events.find((event) => showDeltas || !isDeltaEvent(event.kind.type))?.event_id ?? null;
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}
