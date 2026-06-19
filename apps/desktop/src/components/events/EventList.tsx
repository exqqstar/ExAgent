import { ChevronRight } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import { runtimeEventToInspector, type EventTurnGroup } from "@/lib/eventPresentation";
import { cn } from "@/lib/utils";

export function EventList({
  groups,
  selectedId,
  onSelect
}: {
  groups: EventTurnGroup[];
  selectedId: string | null;
  onSelect: (eventId: string) => void;
}) {
  return (
    <ScrollArea className="min-h-0 flex-1">
      <div className="space-y-3 pr-2">
        {groups.map((group) => (
          <details key={group.turnId ?? "no-turn"} className="group" open>
            <summary className="flex cursor-pointer list-none items-center gap-1.5 rounded-md px-1 py-1 text-left hover:bg-surface-2">
              <ChevronRight className="h-3.5 w-3.5 shrink-0 text-subtle transition-transform group-open:rotate-90" />
              <span className="type-label-sm min-w-0 flex-1 truncate text-muted">
                {group.turnId ? `Turn ${group.turnId}` : "No turn"}
              </span>
              <Badge variant="neutral">{group.events.length}</Badge>
            </summary>
            <div className="mt-1 space-y-1">
              {group.events.map((event) => {
                const item = runtimeEventToInspector(event);
                const selected = event.event_id === selectedId;
                return (
                  <button
                    key={event.event_id}
                    type="button"
                    className={cn(
                      "block w-full rounded-md border border-transparent px-2 py-2 text-left transition-colors",
                      selected ? "border-focus bg-surface-3" : "hover:bg-surface-2"
                    )}
                    onClick={() => onSelect(event.event_id)}
                  >
                    <div className="flex min-w-0 items-center gap-2">
                      <span className="type-label-md min-w-0 flex-1 truncate text-ink">{item.label}</span>
                      <Badge variant={item.tone ?? "neutral"}>{event.event_id}</Badge>
                    </div>
                    <p className="type-body-sm mt-1 line-clamp-2 break-words text-muted">{item.detail}</p>
                  </button>
                );
              })}
            </div>
          </details>
        ))}
      </div>
    </ScrollArea>
  );
}
