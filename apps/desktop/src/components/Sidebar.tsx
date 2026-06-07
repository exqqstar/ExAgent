import { useEffect, useState } from "react";
import {
  Archive,
  CheckCheck,
  ChevronRight,
  Folder,
  FolderPlus,
  FolderOpen,
  GitBranch,
  MoreHorizontal,
  Pencil,
  Pin,
  Plus,
  Search,
  Settings,
  Trash2
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { SettingsDialog } from "@/components/SettingsDialog";
import { exagentClient } from "@/api/exagentClient";
import type { getWorkbenchState } from "@/stores/workbenchStore";
import type { ProjectSummary, SessionStatus, SessionSummary } from "@/types";
import { cn } from "@/lib/utils";

type WorkbenchState = ReturnType<typeof getWorkbenchState>;
type ProjectConfirmation =
  | { type: "archive_project"; project: ProjectSummary }
  | { type: "archive_conversations"; project: ProjectSummary }
  | { type: "remove_project"; project: ProjectSummary }
  | null;

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
  const [renamingProjectId, setRenamingProjectId] = useState<string | null>(null);
  const renamingProject = state.projects.find((project) => project.id === renamingProjectId) ?? null;
  const [renameProjectName, setRenameProjectName] = useState("");
  const [projectConfirmation, setProjectConfirmation] = useState<ProjectConfirmation>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [expandedProjectId, setExpandedProjectId] = useState<string | null>(state.activeProjectId);
  const [projectSessions, setProjectSessions] = useState<Record<string, SessionSummary[]>>({});
  const [loadingProjectSessions, setLoadingProjectSessions] = useState<Record<string, boolean>>({});
  const [projectSessionErrors, setProjectSessionErrors] = useState<Record<string, string>>({});

  useEffect(() => {
    setRenameTitle(renamingSession?.title ?? "");
  }, [renamingSession?.title]);

  useEffect(() => {
    setRenameProjectName(renamingProject?.name ?? "");
  }, [renamingProject?.name]);

  useEffect(() => {
    setExpandedProjectId(state.activeProjectId);
  }, [state.activeProjectId]);

  useEffect(() => {
    if (!state.activeProjectId) {
      return;
    }
    setProjectSessions((current) => ({
      ...current,
      [state.activeProjectId as string]: state.sessions
    }));
  }, [state.activeProjectId, state.sessions]);

  useEffect(() => {
    setProjectSessions(state.activeProjectId ? { [state.activeProjectId]: state.sessions } : {});
    setProjectSessionErrors({});
  }, [state.search]);

  function toggleProject(projectId: string) {
    setExpandedProjectId((current) => (current === projectId ? null : projectId));
    if (expandedProjectId !== projectId) {
      void loadProjectSessions(projectId);
    }
  }

  async function loadProjectSessions(projectId: string) {
    if (projectId === state.activeProjectId) {
      setProjectSessions((current) => ({
        ...current,
        [projectId]: state.sessions
      }));
      return;
    }
    if (projectSessions[projectId] || loadingProjectSessions[projectId]) {
      return;
    }
    setLoadingProjectSessions((current) => ({ ...current, [projectId]: true }));
    setProjectSessionErrors((current) => {
      const { [projectId]: _ignored, ...rest } = current;
      return rest;
    });
    try {
      const threads = state.search
        ? await exagentClient.listThreads(projectId, false, state.search)
        : await exagentClient.reindexProject(projectId);
      setProjectSessions((current) => ({
        ...current,
        [projectId]: threads.map(exagentClient.threadRecordToSession)
      }));
    } catch (error) {
      setProjectSessionErrors((current) => ({
        ...current,
        [projectId]: errorMessage(error)
      }));
    } finally {
      setLoadingProjectSessions((current) => ({ ...current, [projectId]: false }));
    }
  }

  function openProjectSession(session: SessionSummary) {
    if (session.projectId === state.activeProjectId) {
      void state.openSession(session.id);
      return;
    }
    void state.selectProject(session.projectId, session.id);
  }

  function startProjectSession(project: ProjectSummary) {
    void state.startSession(project.id);
  }

  async function revealProject(project: ProjectSummary) {
    try {
      await exagentClient.revealProjectInFileManager(project.path);
    } catch (error) {
      setProjectSessionErrors((current) => ({
        ...current,
        [project.id]: errorMessage(error)
      }));
    }
  }

  function confirmProjectAction(confirmation: Exclude<ProjectConfirmation, null>) {
    switch (confirmation.type) {
      case "archive_project":
        void state.archiveProject(confirmation.project.id);
        break;
      case "archive_conversations":
        void state.archiveProjectConversations(confirmation.project.id);
        break;
      case "remove_project":
        void state.removeProject(confirmation.project.id);
        break;
    }
    setProjectConfirmation(null);
  }

  const confirmationCopy = projectConfirmation ? projectConfirmationText(projectConfirmation) : null;

  return (
    <div className="flex h-full flex-col overflow-hidden">
      <div className="space-y-3 p-3">
        <div className="flex items-center justify-between gap-2">
          <div className="min-w-0">
            <p className="type-label-sm tracking-normal text-muted">Project</p>
            <h1 className="type-title-md truncate text-ink">{activeProject?.name ?? "No project"}</h1>
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
            <p className="type-label-sm tracking-normal text-muted">Projects</p>
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
            {state.projects.map((project) => {
              const active = project.id === state.activeProjectId;
              const expanded = project.id === expandedProjectId;
              const sessions = projectSessions[project.id] ?? (active ? state.sessions : []);
              const loadingSessions = loadingProjectSessions[project.id] ?? false;
              const sessionsError = projectSessionErrors[project.id] ?? null;

              return (
                <div key={project.id}>
                  <div
                    className={cn(
                      "group/project flex items-center rounded-md text-muted transition-colors duration-150 hover:bg-surface-2 hover:text-ink focus-within:bg-surface-2 focus-within:text-ink",
                      active && "active-rail bg-surface-2 text-ink"
                    )}
                  >
                    <button
                      type="button"
                      aria-expanded={expanded}
                      onClick={() => toggleProject(project.id)}
                      className="type-body-sm flex min-w-0 flex-1 items-center gap-2 rounded-md py-1.5 pl-2 pr-1 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus"
                    >
                      <ChevronRight
                        className={cn(
                          "h-3.5 w-3.5 shrink-0 text-subtle transition-transform duration-300 ease-out motion-reduce:transition-none",
                          expanded && "rotate-90 text-muted"
                        )}
                      />
                      <Folder className="h-4 w-4 shrink-0" />
                      <span className="min-w-0 flex-1 truncate">{project.name}</span>
                    </button>
                    <div className="mr-1 flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover/project:opacity-100 group-focus-within/project:opacity-100">
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon"
                            className="h-6 w-6"
                            aria-label={`New session for ${project.name}`}
                            onClick={() => startProjectSession(project)}
                          >
                            <Plus className="h-3.5 w-3.5" />
                          </Button>
                        </TooltipTrigger>
                        <TooltipContent>New session</TooltipContent>
                      </Tooltip>
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon"
                            className="h-6 w-6"
                            aria-label={`Project actions for ${project.name}`}
                          >
                            <MoreHorizontal className="h-3.5 w-3.5" />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end" className="min-w-56">
                          <DropdownMenuItem onSelect={() => void state.pinProject(project.id, !project.pinned)}>
                            <Pin className="mr-2 h-4 w-4" />
                            {project.pinned ? "Unpin project" : "Pin project"}
                          </DropdownMenuItem>
                          <DropdownMenuItem onSelect={() => void revealProject(project)}>
                            <FolderOpen className="mr-2 h-4 w-4" />
                            Show in Finder
                          </DropdownMenuItem>
                          <DropdownMenuItem onSelect={() => void state.createProjectWorktree(project.id)}>
                            <GitBranch className="mr-2 h-4 w-4" />
                            Create permanent worktree
                          </DropdownMenuItem>
                          <DropdownMenuItem onSelect={() => setRenamingProjectId(project.id)}>
                            <Pencil className="mr-2 h-4 w-4" />
                            Rename project
                          </DropdownMenuItem>
                          <DropdownMenuItem disabled>
                            <CheckCheck className="mr-2 h-4 w-4" />
                            Mark all as read
                          </DropdownMenuItem>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem
                            onSelect={() => setProjectConfirmation({ type: "archive_conversations", project })}
                          >
                            <Archive className="mr-2 h-4 w-4" />
                            Archive conversations
                          </DropdownMenuItem>
                          <DropdownMenuItem onSelect={() => setProjectConfirmation({ type: "archive_project", project })}>
                            <Archive className="mr-2 h-4 w-4" />
                            Archive project
                          </DropdownMenuItem>
                          <DropdownMenuItem
                            onSelect={() => setProjectConfirmation({ type: "remove_project", project })}
                            className="text-danger"
                          >
                            <Trash2 className="mr-2 h-4 w-4" />
                            Remove from sidebar
                          </DropdownMenuItem>
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </div>
                  </div>

                  <div
                    aria-hidden={!expanded}
                    className={cn(
                      "grid transition-[grid-template-rows,opacity] duration-200 ease-out motion-reduce:transition-none",
                      expanded ? "grid-rows-[1fr] opacity-100" : "pointer-events-none grid-rows-[0fr] opacity-0"
                    )}
                  >
                    <div className="min-h-0 overflow-hidden">
                      <div className="mb-1 ml-2 mr-1.5">
                        <div className="space-y-0.5">
                          {sessions.map((session) => (
                            <div
                              key={session.id}
                              className={cn(
                                "type-label-sm group flex w-full items-center gap-1.5 rounded-md border border-transparent pr-1.5 text-muted transition-colors duration-150 hover:border-border hover:bg-surface-2 hover:text-ink",
                                session.id === state.activeSessionId && "active-rail border-border-strong bg-surface-2 text-ink"
                              )}
                            >
                              <button
                                type="button"
                                tabIndex={expanded ? undefined : -1}
                                onClick={() => openProjectSession(session)}
                                className="flex min-w-0 flex-1 items-center gap-2 rounded-md py-1 pl-6 pr-1 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus"
                              >
                                <span className="min-w-0 flex-1 truncate">{session.title}</span>
                                <Badge variant={statusVariant[session.status]}>{session.status.replace("_", " ")}</Badge>
                              </button>
                              <Tooltip>
                                <TooltipTrigger asChild>
                                  <Button
                                    type="button"
                                    variant="ghost"
                                    size="icon"
                                    tabIndex={expanded ? undefined : -1}
                                    className="h-6 w-6 opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100"
                                    aria-label={`Archive ${session.title}`}
                                    onClick={() => void state.archiveSession(session.id)}
                                  >
                                    <Archive className="h-3.5 w-3.5" />
                                  </Button>
                                </TooltipTrigger>
                                <TooltipContent>Archive session</TooltipContent>
                              </Tooltip>
                              <DropdownMenu>
                                <DropdownMenuTrigger asChild>
                                  <Button
                                    type="button"
                                    variant="ghost"
                                    size="icon"
                                    tabIndex={expanded ? undefined : -1}
                                    className="h-6 w-6 opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100"
                                    aria-label={`Session actions for ${session.title}`}
                                  >
                                    <MoreHorizontal className="h-3.5 w-3.5" />
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
                          {loadingSessions ? (
                            <p className="type-body-sm px-2 py-1.5 text-subtle">Loading sessions...</p>
                          ) : null}
                          {sessionsError ? (
                            <p className="type-body-sm px-2 py-1.5 text-danger">{sessionsError}</p>
                          ) : null}
                          {!loadingSessions && !sessionsError && sessions.length === 0 ? (
                            <p className="type-body-sm px-2 py-1.5 text-subtle">No sessions</p>
                          ) : null}
                        </div>
                      </div>
                    </div>
                  </div>
                </div>
              );
            })}
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

      <Dialog open={renamingProject !== null} onOpenChange={(open) => !open && setRenamingProjectId(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Rename project</DialogTitle>
            <DialogDescription>Set a local name for this sidebar project.</DialogDescription>
          </DialogHeader>
          <form
            className="space-y-4"
            onSubmit={(event) => {
              event.preventDefault();
              if (!renamingProject) {
                return;
              }
              void state.renameProject(renamingProject.id, renameProjectName);
              setRenamingProjectId(null);
            }}
          >
            <Input
              autoFocus
              aria-label="Project name"
              value={renameProjectName}
              onChange={(event) => setRenameProjectName(event.target.value)}
            />
            <DialogFooter>
              <Button type="button" variant="secondary" onClick={() => setRenamingProjectId(null)}>
                Cancel
              </Button>
              <Button type="submit" disabled={!renameProjectName.trim()}>
                Save
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <AlertDialog open={projectConfirmation !== null} onOpenChange={(open) => !open && setProjectConfirmation(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{confirmationCopy?.title}</AlertDialogTitle>
            <AlertDialogDescription>{confirmationCopy?.description}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className={projectConfirmation?.type === "remove_project" ? "bg-danger text-white hover:brightness-110" : undefined}
              onClick={() => {
                if (projectConfirmation) {
                  confirmProjectAction(projectConfirmation);
                }
              }}
            >
              {confirmationCopy?.action}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <SettingsDialog open={settingsOpen} onOpenChange={setSettingsOpen} />
    </div>
  );
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function projectConfirmationText(confirmation: Exclude<ProjectConfirmation, null>) {
  switch (confirmation.type) {
    case "archive_project":
      return {
        title: `Archive ${confirmation.project.name}?`,
        description: "This hides the project from the sidebar. It does not delete the folder or conversation files.",
        action: "Archive project"
      };
    case "archive_conversations":
      return {
        title: `Archive conversations in ${confirmation.project.name}?`,
        description: "This hides this project's sessions from the default list. Runtime rollout files stay on disk.",
        action: "Archive conversations"
      };
    case "remove_project":
      return {
        title: `Remove ${confirmation.project.name} from the sidebar?`,
        description: "This removes the project from the desktop registry only. It does not delete files from disk.",
        action: "Remove from sidebar"
      };
  }
}
