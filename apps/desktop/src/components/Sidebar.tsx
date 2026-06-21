import { useEffect, useState } from "react";
import {
  Archive,
  CheckCheck,
  ChevronRight,
  Folder,
  FolderPlus,
  FolderOpen,
  GitBranch,
  GitCompareArrows,
  MoreHorizontal,
  Pencil,
  Pin,
  Plus,
  Search,
  Settings,
  Trash2,
  Workflow
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
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { SettingsDialog } from "@/components/SettingsDialog";
import { exagentClient } from "@/api/exagentClient";
import type { getWorkbenchState } from "@/stores/workbenchStore";
import type { ProjectSummary, SessionStatus, SessionSummary } from "@/types";
import { useI18n, type TranslationKey } from "@/lib/i18n";
import { cn } from "@/lib/utils";

type WorkbenchState = ReturnType<typeof getWorkbenchState>;
type SidebarView = "chat" | "workflows";
type ProjectConfirmation =
  | { type: "archive_project"; project: ProjectSummary }
  | { type: "archive_conversations"; project: ProjectSummary }
  | { type: "remove_project"; project: ProjectSummary }
  | null;
type SessionSearchResult = {
  session: SessionSummary;
  project: ProjectSummary;
};

const statusVariant: Record<SessionStatus, "neutral" | "success" | "warning" | "danger"> = {
  idle: "neutral",
  running: "success",
  awaiting_approval: "warning",
  failed: "danger",
  archived: "neutral"
};

export function Sidebar({
  state,
  activeView = "chat",
  onSelectView
}: {
  state: WorkbenchState;
  activeView?: SidebarView;
  onSelectView?: (view: SidebarView) => void;
}) {
  const { t } = useI18n();
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
  const [sessionSearchOpen, setSessionSearchOpen] = useState(false);
  const [sessionSearchQuery, setSessionSearchQuery] = useState("");
  const [sessionSearchResults, setSessionSearchResults] = useState<SessionSearchResult[]>([]);
  const [sessionSearchLoading, setSessionSearchLoading] = useState(false);
  const [sessionSearchError, setSessionSearchError] = useState<string | null>(null);
  const visibleProjects = state.projects.filter((project) => !isPersonalProject(project));
  const normalizedSessionSearchQuery = sessionSearchQuery.trim().toLowerCase();
  const searchableSessions = sessionSearchResults.filter((result) =>
    result.session.title.toLowerCase().includes(normalizedSessionSearchQuery)
  );

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

  useEffect(() => {
    if (!sessionSearchOpen) {
      return;
    }

    let canceled = false;
    const projects = state.projects.filter((project) => !project.archived);
    setSessionSearchLoading(true);
    setSessionSearchError(null);

    async function loadRecentSessions() {
      try {
        const groupedResults = await Promise.all(
          projects.map(async (project) => {
            const sessions =
              project.id === state.activeProjectId
                ? state.sessions
                : (await exagentClient.listThreads(project.id, false, null)).map(exagentClient.threadRecordToSession);
            return sessions.map((session) => ({ session, project }));
          })
        );
        if (canceled) {
          return;
        }
        setSessionSearchResults(
          groupedResults
            .flat()
            .sort((left, right) => (right.session.createdAt ?? 0) - (left.session.createdAt ?? 0))
        );
      } catch (error) {
        if (!canceled) {
          setSessionSearchError(errorMessage(error));
        }
      } finally {
        if (!canceled) {
          setSessionSearchLoading(false);
        }
      }
    }

    void loadRecentSessions();
    return () => {
      canceled = true;
    };
  }, [sessionSearchOpen, state.activeProjectId, state.projects, state.sessions]);

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
    onSelectView?.("chat");
    if (session.projectId === state.activeProjectId) {
      void state.openSession(session.id);
      return;
    }
    void state.selectProject(session.projectId, session.id);
  }

  function startProjectSession(project: ProjectSummary) {
    onSelectView?.("chat");
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

  const confirmationCopy = projectConfirmation ? projectConfirmationText(projectConfirmation, t) : null;

  function forkLabelForSession(session: SessionSummary) {
    if (!session.forkPointTurnId) {
      return null;
    }
    return t("sessions.forkedFromTurn").replace("{turn}", forkTurnLabel(session.forkPointTurnId));
  }

  function renderSessionBranch(node: SessionBranchNode, expanded: boolean) {
    const forkLabel = forkLabelForSession(node.session);
    return (
      <div key={node.session.id} className={cn("min-w-0", node.children.length > 0 && "space-y-0.5")}>
        {renderSessionRow(node.session, expanded, forkLabel)}
        {node.children.length > 0 ? (
          <div
            data-session-branch-group
            className="ml-[7px] min-w-0 space-y-0.5 overflow-hidden border-l border-border pl-2.5"
          >
            {node.children.map((child) => renderSessionBranch(child, expanded))}
          </div>
        ) : null}
      </div>
    );
  }

  function renderSessionRow(session: SessionSummary, expanded: boolean, forkLabel: string | null) {
    const sessionButtonLabel = forkLabel
      ? formatText(t("sidebar.forkedSessionLabel"), { session: session.title, fork: forkLabel })
      : undefined;
    return (
      <div
        className={cn(
          "type-label-sm group flex w-full items-center gap-1.5 rounded-lg border border-transparent pr-1.5 text-muted transition-colors duration-150 hover:border-border hover:bg-surface-2 hover:text-ink",
          session.id === state.activeSessionId && "active-rail border-border-strong bg-surface-2 text-ink"
        )}
      >
        <button
          type="button"
          tabIndex={expanded ? undefined : -1}
          aria-label={sessionButtonLabel}
          onClick={() => openProjectSession(session)}
          className={cn(
            "flex min-w-0 flex-1 items-center gap-2 rounded-lg py-1 pr-1 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus",
            forkLabel ? "pl-2" : "pl-6"
          )}
        >
          <span className="min-w-0 flex-1 overflow-hidden">
            <span className="block truncate">{session.title}</span>
            {forkLabel ? (
              <Tooltip>
                <TooltipTrigger asChild>
                  <span
                    className="mt-0.5 flex min-w-0 items-center gap-1 text-subtle"
                    aria-label={forkLabel}
                  >
                    <GitBranch aria-hidden className="h-3 w-3 shrink-0" />
                    <span className="truncate">{forkLabel}</span>
                  </span>
                </TooltipTrigger>
                <TooltipContent>{forkLabel}</TooltipContent>
              </Tooltip>
            ) : null}
          </span>
          {session.status !== "idle" ? (
            <Badge variant={statusVariant[session.status]}>{sessionStatusLabel(session.status, t)}</Badge>
          ) : null}
        </button>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              tabIndex={expanded ? undefined : -1}
              className="h-6 w-6 opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100"
              aria-label={formatText(t("sidebar.archiveSessionFor"), { session: session.title })}
              onClick={() => void state.archiveSession(session.id)}
            >
              <Archive className="h-3.5 w-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>{t("sidebar.archiveSession")}</TooltipContent>
        </Tooltip>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              tabIndex={expanded ? undefined : -1}
              className="h-6 w-6 opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100"
              aria-label={formatText(t("sidebar.sessionActionsFor"), { session: session.title })}
            >
              <MoreHorizontal className="h-3.5 w-3.5" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {session.forkParentThreadId && session.forkPointTurnId ? (
              <>
                <DropdownMenuItem onSelect={() => void state.openBranchCompare(session.id, session.projectId)}>
                  <GitCompareArrows className="mr-2 h-4 w-4" />
                  {t("sidebar.compareWithParent")}
                </DropdownMenuItem>
                <DropdownMenuSeparator />
              </>
            ) : null}
            <DropdownMenuItem
              onSelect={() => {
                setRenamingSessionId(session.id);
              }}
            >
              <Pencil className="mr-2 h-4 w-4" />
              {t("sidebar.renameSession")}
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => void state.pinSession(session.id, !session.pinned)}>
              <Pin className="mr-2 h-4 w-4" />
              {session.pinned ? t("sidebar.unpinSession") : t("sidebar.pinSession")}
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => void state.archiveSession(session.id)}>
              <Archive className="mr-2 h-4 w-4" />
              {t("sidebar.archiveSession")}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col overflow-hidden">
      <span className="sr-only">{t("sidebar.project")}</span>
      <div className="sidebar-quick-actions p-1.5">
        <button
          type="button"
          className="sidebar-action-row group"
          aria-label={t("sidebar.newChat")}
          onClick={() => {
            onSelectView?.("chat");
            void state.startPersonalSession();
          }}
        >
          <Pencil className="h-3.5 w-3.5 shrink-0 text-muted transition-colors group-hover:text-ink" />
          <span className="min-w-0 flex-1 truncate">{t("sidebar.newChat")}</span>
        </button>
        <button
          type="button"
          className="sidebar-action-row group"
          aria-label={t("sidebar.searchDialog.placeholder")}
          onClick={() => setSessionSearchOpen(true)}
        >
          <Search className="h-3.5 w-3.5 shrink-0 text-muted transition-colors group-hover:text-ink" />
          <span className="min-w-0 flex-1 truncate">{t("sidebar.search")}</span>
        </button>
        <button
          type="button"
          className={cn("sidebar-action-row group", activeView === "workflows" && "active-rail bg-surface-2")}
          aria-label={t("sidebar.workflows")}
          aria-current={activeView === "workflows" ? "page" : undefined}
          onClick={() => onSelectView?.("workflows")}
        >
          <Workflow className="h-3.5 w-3.5 shrink-0 text-muted transition-colors group-hover:text-ink" />
          <span className="min-w-0 flex-1 truncate">{t("sidebar.workflows")}</span>
          <Badge variant="warning" className="shrink-0">
            WIP
          </Badge>
        </button>
      </div>

      <ScrollArea className="min-h-0 flex-1 pt-1">
        <section className="p-2">
          <div className="flex items-center justify-between gap-2 px-2 py-1.5">
            <p className="type-label-sm tracking-normal text-muted">{t("sidebar.projects")}</p>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  size="icon"
                  variant="ghost"
                  className="h-7 w-7"
                  aria-label={t("sidebar.addProject")}
                  onClick={() => void state.addProject()}
                >
                  <FolderPlus className="h-4 w-4" />
                </Button>
              </TooltipTrigger>
              <TooltipContent>{t("sidebar.addProject")}</TooltipContent>
            </Tooltip>
          </div>
          <div className="space-y-1">
            {visibleProjects.map((project) => {
              const active = project.id === state.activeProjectId;
              const expanded = project.id === expandedProjectId;
              const sessions = projectSessions[project.id] ?? (active ? state.sessions : []);
              const loadingSessions = loadingProjectSessions[project.id] ?? false;
              const sessionsError = projectSessionErrors[project.id] ?? null;

              return (
                <div key={project.id}>
                  <div
                    className={cn(
                      "group/project flex items-center rounded-lg text-muted transition-colors duration-150 hover:bg-surface-2 hover:text-ink focus-within:bg-surface-2 focus-within:text-ink",
                      active && "active-rail bg-surface-2 text-ink"
                    )}
                  >
                    <button
                      type="button"
                      aria-expanded={expanded}
                      onClick={() => toggleProject(project.id)}
                      className="type-body-sm flex min-w-0 flex-1 items-center gap-2 rounded-lg py-1.5 pl-2 pr-1 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus"
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
                            aria-label={formatText(t("sidebar.newSessionFor"), { project: project.name })}
                            onClick={() => startProjectSession(project)}
                          >
                            <Plus className="h-3.5 w-3.5" />
                          </Button>
                        </TooltipTrigger>
                        <TooltipContent>{t("sidebar.newSession")}</TooltipContent>
                      </Tooltip>
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon"
                            className="h-6 w-6"
                            aria-label={formatText(t("sidebar.projectActionsFor"), { project: project.name })}
                          >
                            <MoreHorizontal className="h-3.5 w-3.5" />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end" className="min-w-56">
                          <DropdownMenuItem onSelect={() => void state.pinProject(project.id, !project.pinned)}>
                            <Pin className="mr-2 h-4 w-4" />
                            {project.pinned ? t("sidebar.unpinProject") : t("sidebar.pinProject")}
                          </DropdownMenuItem>
                          <DropdownMenuItem onSelect={() => void revealProject(project)}>
                            <FolderOpen className="mr-2 h-4 w-4" />
                            {t("sidebar.showInFinder")}
                          </DropdownMenuItem>
                          <DropdownMenuItem onSelect={() => void state.createProjectWorktree(project.id)}>
                            <GitBranch className="mr-2 h-4 w-4" />
                            {t("sidebar.createPermanentWorktree")}
                          </DropdownMenuItem>
                          <DropdownMenuItem onSelect={() => setRenamingProjectId(project.id)}>
                            <Pencil className="mr-2 h-4 w-4" />
                            {t("sidebar.renameProject")}
                          </DropdownMenuItem>
                          <DropdownMenuItem disabled>
                            <CheckCheck className="mr-2 h-4 w-4" />
                            {t("sidebar.markAllRead")}
                          </DropdownMenuItem>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem
                            onSelect={() => setProjectConfirmation({ type: "archive_conversations", project })}
                          >
                            <Archive className="mr-2 h-4 w-4" />
                            {t("sidebar.archiveConversations")}
                          </DropdownMenuItem>
                          <DropdownMenuItem onSelect={() => setProjectConfirmation({ type: "archive_project", project })}>
                            <Archive className="mr-2 h-4 w-4" />
                            {t("sidebar.archiveProject")}
                          </DropdownMenuItem>
                          <DropdownMenuItem
                            onSelect={() => setProjectConfirmation({ type: "remove_project", project })}
                            className="text-danger"
                          >
                            <Trash2 className="mr-2 h-4 w-4" />
                            {t("sidebar.removeFromSidebar")}
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
                          {buildSessionBranchRows(sessions).map((node) => renderSessionBranch(node, expanded))}
                          {loadingSessions ? (
                            <p className="type-body-sm px-2 py-1.5 text-subtle">{t("sidebar.loadingSessions")}</p>
                          ) : null}
                          {sessionsError ? (
                            <p className="type-body-sm px-2 py-1.5 text-danger">{sessionsError}</p>
                          ) : null}
                          {!loadingSessions && !sessionsError && sessions.length === 0 ? (
                            <p className="type-body-sm px-2 py-1.5 text-subtle">{t("sidebar.noSessions")}</p>
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

      <div className="sidebar-footer p-2">
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              className="w-full justify-start"
              aria-label={t("sidebar.openSettings")}
              onClick={() => setSettingsOpen(true)}
            >
              <Settings className="h-4 w-4" />
              {t("sidebar.settings")}
            </Button>
          </TooltipTrigger>
          <TooltipContent>{t("sidebar.settings")}</TooltipContent>
        </Tooltip>
      </div>

      <Dialog open={sessionSearchOpen} onOpenChange={setSessionSearchOpen}>
        <DialogContent className="w-[min(560px,calc(100vw-32px))] gap-0 overflow-hidden p-0">
          <DialogHeader className="border-b border-border px-4 py-3">
            <DialogTitle>{t("sidebar.searchDialog.title")}</DialogTitle>
            <DialogDescription>{t("sidebar.searchDialog.description")}</DialogDescription>
          </DialogHeader>
          <div className="border-b border-border p-3">
            <div className="relative">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-subtle" />
              <Input
                className="pl-8"
                placeholder={t("sidebar.searchDialog.placeholder")}
                aria-label={t("sidebar.searchDialog.placeholder")}
                value={sessionSearchQuery}
                onChange={(event) => setSessionSearchQuery(event.target.value)}
              />
            </div>
          </div>
          <div className="max-h-[360px] overflow-y-auto p-2">
            {searchableSessions.length > 0 ? (
              <div className="space-y-1">
                {searchableSessions.map(({ session, project }) => (
                  <button
                    key={session.id}
                    type="button"
                    className="type-body-sm flex w-full items-center justify-between gap-3 rounded-lg px-2.5 py-2 text-left text-muted transition-colors hover:bg-surface-2 hover:text-ink focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus"
                    onClick={() => {
                      setSessionSearchOpen(false);
                      setSessionSearchQuery("");
                      openProjectSession(session);
                    }}
                  >
                    <span className="min-w-0 flex-1 overflow-hidden">
                      <span className="block truncate">{session.title}</span>
                      <span className="type-label-sm block truncate text-subtle">
                        {isPersonalProject(project) ? t("sidebar.searchDialog.noProject") : project.name}
                      </span>
                    </span>
                    <span className="type-label-sm shrink-0 text-subtle">{session.updatedAt}</span>
                  </button>
                ))}
              </div>
            ) : sessionSearchLoading ? (
              <p className="type-body-sm px-2.5 py-6 text-center text-subtle">{t("sidebar.loadingSessions")}</p>
            ) : sessionSearchError ? (
              <p className="type-body-sm px-2.5 py-6 text-center text-danger">{sessionSearchError}</p>
            ) : (
              <p className="type-body-sm px-2.5 py-6 text-center text-subtle">{t("sidebar.searchDialog.noMatches")}</p>
            )}
          </div>
        </DialogContent>
      </Dialog>

      <Dialog open={renamingSession !== null} onOpenChange={(open) => !open && setRenamingSessionId(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("sidebar.renameSession.title")}</DialogTitle>
            <DialogDescription>{t("sidebar.renameSession.description")}</DialogDescription>
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
              aria-label={t("sidebar.renameSession.field")}
              value={renameTitle}
              onChange={(event) => setRenameTitle(event.target.value)}
            />
            <DialogFooter>
              <Button type="button" variant="secondary" onClick={() => setRenamingSessionId(null)}>
                {t("common.cancel")}
              </Button>
              <Button type="submit" disabled={!renameTitle.trim()}>
                {t("common.save")}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <Dialog open={renamingProject !== null} onOpenChange={(open) => !open && setRenamingProjectId(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("sidebar.renameProject.title")}</DialogTitle>
            <DialogDescription>{t("sidebar.renameProject.description")}</DialogDescription>
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
              aria-label={t("sidebar.renameProject.field")}
              value={renameProjectName}
              onChange={(event) => setRenameProjectName(event.target.value)}
            />
            <DialogFooter>
              <Button type="button" variant="secondary" onClick={() => setRenamingProjectId(null)}>
                {t("common.cancel")}
              </Button>
              <Button type="submit" disabled={!renameProjectName.trim()}>
                {t("common.save")}
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
            <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
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

type SessionBranchNode = {
  session: SessionSummary;
  children: SessionBranchNode[];
};

function buildSessionBranchRows(sessions: SessionSummary[]): SessionBranchNode[] {
  const sessionsById = new Map(sessions.map((session) => [session.id, session]));
  const childrenByParent = new Map<string, SessionSummary[]>();
  const childIds = new Set<string>();

  for (const session of sessions) {
    const parentId = session.forkParentThreadId;
    if (!parentId || !sessionsById.has(parentId)) {
      continue;
    }
    childIds.add(session.id);
    const children = childrenByParent.get(parentId) ?? [];
    children.push(session);
    childrenByParent.set(parentId, children);
  }

  const visited = new Set<string>();
  const toNode = (session: SessionSummary): SessionBranchNode => {
    visited.add(session.id);
    const children = (childrenByParent.get(session.id) ?? [])
      .filter((child) => !visited.has(child.id))
      .map(toNode);
    return { session, children };
  };

  const roots = sessions.filter((session) => !childIds.has(session.id)).map(toNode);

  for (const session of sessions) {
    if (!visited.has(session.id)) {
      roots.push({ session, children: [] });
    }
  }

  return roots;
}

function isPersonalProject(project: ProjectSummary) {
  return project.name === "Personal" && /[\\/]conversations$/i.test(project.path);
}

function forkTurnLabel(turnId: string) {
  return turnId.match(/(\d+)$/)?.[1] ?? turnId;
}

function formatText(template: string, values: Record<string, string>) {
  return Object.entries(values).reduce(
    (result, [key, value]) => result.replace(`{${key}}`, value),
    template
  );
}

function projectConfirmationText(
  confirmation: Exclude<ProjectConfirmation, null>,
  t: (key: TranslationKey) => string
) {
  switch (confirmation.type) {
    case "archive_project":
      return {
        title: formatText(t("sidebar.confirm.archiveProject.title"), { project: confirmation.project.name }),
        description: t("sidebar.confirm.archiveProject.description"),
        action: t("sidebar.confirm.archiveProject.action")
      };
    case "archive_conversations":
      return {
        title: formatText(t("sidebar.confirm.archiveConversations.title"), { project: confirmation.project.name }),
        description: t("sidebar.confirm.archiveConversations.description"),
        action: t("sidebar.confirm.archiveConversations.action")
      };
    case "remove_project":
      return {
        title: formatText(t("sidebar.confirm.removeProject.title"), { project: confirmation.project.name }),
        description: t("sidebar.confirm.removeProject.description"),
        action: t("sidebar.confirm.removeProject.action")
      };
  }
}

function sessionStatusLabel(status: SessionStatus, t: (key: TranslationKey) => string) {
  switch (status) {
    case "running":
      return t("status.session.running");
    case "awaiting_approval":
      return t("status.session.awaitingApproval");
    case "failed":
      return t("status.session.failed");
    case "archived":
      return t("status.session.archived");
    case "idle":
    default:
      return t("status.session.idle");
  }
}
