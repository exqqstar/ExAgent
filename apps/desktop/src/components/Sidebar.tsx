import { useEffect, useState } from "react";
import {
  Archive,
  Folder,
  FolderPlus,
  MoreHorizontal,
  Pencil,
  Pin,
  Plus,
  Search,
  Settings
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { SettingsDialog } from "@/components/SettingsDialog";
import type { getWorkbenchState } from "@/stores/workbenchStore";
import type { SessionStatus } from "@/types";
import { cn } from "@/lib/utils";

type WorkbenchState = ReturnType<typeof getWorkbenchState>;

const statusVariant: Record<SessionStatus, "neutral" | "success" | "warning" | "danger"> = {
  idle: "neutral",
  running: "success",
  awaiting_approval: "warning",
  failed: "danger",
  archived: "neutral"
};

export function Sidebar({ state }: { state: WorkbenchState }) {
  const activeProject = state.projects.find((project) => project.id === state.activeProjectId);
  const [renamingSessionId, setRenamingSessionId] = useState<string | null>(null);
  const renamingSession = state.sessions.find((session) => session.id === renamingSessionId) ?? null;
  const [renameTitle, setRenameTitle] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);

  useEffect(() => {
    setRenameTitle(renamingSession?.title ?? "");
  }, [renamingSession?.title]);

  return (
    <div className="flex h-full flex-col">
      <div className="space-y-3 p-3">
        <div className="flex items-center justify-between gap-2">
          <div className="min-w-0">
            <p className="text-xs font-medium uppercase tracking-normal text-subtle">Project</p>
            <h1 className="truncate text-lg font-semibold text-ink">{activeProject?.name ?? "No project"}</h1>
          </div>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                size="icon"
                variant="secondary"
                aria-label="New session"
                onClick={() => void state.startSession()}
              >
                <Plus className="h-4 w-4" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>New session</TooltipContent>
          </Tooltip>
        </div>

        <div className="relative">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-subtle" />
          <Input
            className="pl-8"
            placeholder="Search sessions"
            aria-label="Search sessions"
            value={state.search}
            onChange={(event) => void state.setSearch(event.target.value)}
          />
        </div>
      </div>

      <Separator />

      <ScrollArea className="min-h-0 flex-1">
        <section className="p-2">
          <div className="flex items-center justify-between gap-2 px-2 py-1.5">
            <p className="text-xs font-medium uppercase tracking-normal text-subtle">Projects</p>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  size="icon"
                  variant="ghost"
                  className="h-7 w-7"
                  aria-label="Add project"
                  onClick={() => void state.addProject()}
                >
                  <FolderPlus className="h-4 w-4" />
                </Button>
              </TooltipTrigger>
              <TooltipContent>Add project</TooltipContent>
            </Tooltip>
          </div>
          <div className="space-y-1">
            {state.projects.map((project) => (
              <button
                key={project.id}
                type="button"
                onClick={() => void state.selectProject(project.id)}
                className={cn(
                  "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm text-muted hover:bg-surface-2 hover:text-ink",
                  project.id === state.activeProjectId && "bg-surface-2 text-ink"
                )}
              >
                <Folder className="h-4 w-4 shrink-0" />
                <span className="min-w-0 flex-1 truncate">{project.name}</span>
              </button>
            ))}
          </div>
        </section>

        <section className="p-2 pt-0">
          <p className="px-2 py-1.5 text-xs font-medium uppercase tracking-normal text-subtle">Sessions</p>
          <div className="space-y-1">
            {state.sessions.map((session) => (
              <div
                key={session.id}
                role="button"
                tabIndex={0}
                onClick={() => void state.openSession(session.id)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" || event.key === " ") {
                    void state.openSession(session.id);
                  }
                }}
                className={cn(
                  "group flex w-full items-start gap-2 rounded-md px-2 py-2 text-left text-sm hover:bg-surface-2",
                  session.id === state.activeSessionId && "bg-surface-2"
                )}
              >
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="truncate font-medium text-ink">{session.title}</span>
                    <Badge variant={statusVariant[session.status]}>{session.status.replace("_", " ")}</Badge>
                  </div>
                  <p className="mt-1 truncate text-xs text-subtle">{session.updatedAt}</p>
                </div>
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="h-7 w-7 opacity-0 group-hover:opacity-100"
                      aria-label={`Session actions for ${session.title}`}
                      onClick={(event) => event.stopPropagation()}
                    >
                      <MoreHorizontal className="h-4 w-4" />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end">
                    <DropdownMenuItem
                      onSelect={() => {
                        setRenamingSessionId(session.id);
                      }}
                    >
                      <Pencil className="mr-2 h-4 w-4" />
                      Rename session
                    </DropdownMenuItem>
                    <DropdownMenuItem onSelect={() => void state.pinSession(session.id, !session.pinned)}>
                      <Pin className="mr-2 h-4 w-4" />
                      {session.pinned ? "Unpin session" : "Pin session"}
                    </DropdownMenuItem>
                    <DropdownMenuItem onSelect={() => void state.archiveSession(session.id)}>
                      <Archive className="mr-2 h-4 w-4" />
                      Archive session
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
            ))}
            {state.sessions.length === 0 ? (
              <p className="px-2 py-2 text-sm text-muted">No sessions</p>
            ) : null}
          </div>
        </section>
      </ScrollArea>

      <Separator />

      <div className="p-2">
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              className="w-full justify-start"
              aria-label="Open settings"
              onClick={() => setSettingsOpen(true)}
            >
              <Settings className="h-4 w-4" />
              Settings
            </Button>
          </TooltipTrigger>
          <TooltipContent>Settings</TooltipContent>
        </Tooltip>
      </div>

      <Dialog open={renamingSession !== null} onOpenChange={(open) => !open && setRenamingSessionId(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Rename session</DialogTitle>
            <DialogDescription>Set a local title for this project's session list.</DialogDescription>
          </DialogHeader>
          <form
            className="space-y-4"
            onSubmit={(event) => {
              event.preventDefault();
              if (!renamingSession) {
                return;
              }
              void state.renameSession(renamingSession.id, renameTitle);
              setRenamingSessionId(null);
            }}
          >
            <Input
              autoFocus
              aria-label="Session title"
              value={renameTitle}
              onChange={(event) => setRenameTitle(event.target.value)}
            />
            <DialogFooter>
              <Button type="button" variant="secondary" onClick={() => setRenamingSessionId(null)}>
                Cancel
              </Button>
              <Button type="submit" disabled={!renameTitle.trim()}>
                Save
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <SettingsDialog open={settingsOpen} onOpenChange={setSettingsOpen} />
    </div>
  );
}
