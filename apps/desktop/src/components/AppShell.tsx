import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent
} from "react";
import { ArrowLeft, ArrowRight, Database, PanelRight, ShieldAlert, SidebarIcon } from "lucide-react";
import { AgentThreadViewer } from "@/components/AgentThreadViewer";
import { ApprovalInbox } from "@/components/ApprovalInbox";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle, SheetTrigger } from "@/components/ui/sheet";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { ChatView } from "@/components/ChatView";
import { Inspector } from "@/components/Inspector";
import { MemoryInspector } from "@/components/memory/MemoryInspector";
import { Sidebar } from "@/components/Sidebar";
import { loadWorkbench, useWorkbenchStore } from "@/stores/workbenchStore";
import type { AgentNode } from "@/types";
import { useI18n } from "@/lib/i18n";
import { cn } from "@/lib/utils";

const DESKTOP_SIDEBAR_DEFAULT_WIDTH = 280;
const DESKTOP_SIDEBAR_MIN_WIDTH = 240;
const DESKTOP_SIDEBAR_MAX_WIDTH = 420;
const DESKTOP_SIDEBAR_COLLAPSE_WIDTH = 220;

export function AppShell() {
  const { t } = useI18n();
  const workbench = useWorkbenchStore();
  const activeSession = workbench.sessions.find((session) => session.id === workbench.activeSessionId);
  const selectedAgent = workbench.selectedAgentThreadId
    ? findAgentNode(workbench.agents, workbench.selectedAgentThreadId)
    : null;
  const selectedAgentTokenUsage = selectedAgent
    ? workbench.tokenUsageByThreadId[selectedAgent.threadId] ?? null
    : null;
  const activeStatus = activeSession?.status ?? "idle";
  const runtimeModel = workbench.selectedModel?.model_id ?? workbench.runtimeSettings?.default_model ?? "default";
  const shellRef = useRef<HTMLDivElement | null>(null);
  const resizingDesktopSidebarRef = useRef(false);
  const desktopSidebarWidthBeforeResizeRef = useRef(DESKTOP_SIDEBAR_DEFAULT_WIDTH);
  const [desktopSidebarWidth, setDesktopSidebarWidth] = useState(DESKTOP_SIDEBAR_DEFAULT_WIDTH);
  const [desktopSidebarCollapsed, setDesktopSidebarCollapsed] = useState(false);
  const [resizingDesktopSidebar, setResizingDesktopSidebar] = useState(false);
  const [memoryOpen, setMemoryOpen] = useState(false);
  const pendingApprovalCount = workbench.pendingApprovals.length;
  const chromeSidebarWidth = desktopSidebarCollapsed ? 164 : desktopSidebarWidth;
  const approvalInboxLabel = `${t("approvals.inbox.title")}, ${pendingApprovalCount} ${t("approvals.inbox.pending")} ${
    pendingApprovalCount === 1 ? t("approvals.inbox.approvalSingular") : t("approvals.inbox.approvalPlural")
  }`;

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) {
      return;
    }
    void import("@tauri-apps/api/window")
      .then(async ({ getCurrentWindow }) => {
        const window = getCurrentWindow();
        await window.setTitle("");
        await window.setTitleBarStyle("overlay");
      })
      .catch(() => undefined);
  }, []);

  const resizeDesktopSidebar = useCallback((clientX: number) => {
    const shellLeft = shellRef.current?.getBoundingClientRect().left ?? 0;
    const nextWidth = clientX - shellLeft;

    if (nextWidth < DESKTOP_SIDEBAR_COLLAPSE_WIDTH) {
      resizingDesktopSidebarRef.current = false;
      setDesktopSidebarWidth(desktopSidebarWidthBeforeResizeRef.current);
      setDesktopSidebarCollapsed(true);
      setResizingDesktopSidebar(false);
      return;
    }

    setDesktopSidebarCollapsed(false);
    setDesktopSidebarWidth(clamp(nextWidth, DESKTOP_SIDEBAR_MIN_WIDTH, DESKTOP_SIDEBAR_MAX_WIDTH));
  }, []);

  useEffect(() => {
    if (workbench.loading && workbench.projects.length === 0) {
      void loadWorkbench();
    }
  }, [workbench.loading, workbench.projects.length]);

  useEffect(() => {
    if (!workbench.compareThreadId) {
      return;
    }

    function closeCompareOnEscape(event: KeyboardEvent) {
      if (event.key !== "Escape") {
        return;
      }
      event.preventDefault();
      workbench.closeCompareView();
    }

    document.addEventListener("keydown", closeCompareOnEscape);
    return () => document.removeEventListener("keydown", closeCompareOnEscape);
  }, [workbench.compareThreadId, workbench.closeCompareView]);

  useEffect(() => {
    function startPersonalSessionShortcut(event: KeyboardEvent) {
      if (event.defaultPrevented || event.altKey || event.shiftKey) {
        return;
      }
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== "n") {
        return;
      }
      event.preventDefault();
      void workbench.startPersonalSession();
    }

    document.addEventListener("keydown", startPersonalSessionShortcut);
    return () => document.removeEventListener("keydown", startPersonalSessionShortcut);
  }, [workbench.startPersonalSession]);

  useEffect(() => {
    if (!resizingDesktopSidebar) {
      return;
    }

    const previousCursor = document.body.style.cursor;
    const previousUserSelect = document.body.style.userSelect;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";

    function handlePointerMove(event: PointerEvent) {
      if (!resizingDesktopSidebarRef.current) {
        return;
      }
      resizeDesktopSidebar(event.clientX);
    }

    function handleMouseMove(event: MouseEvent) {
      if (!resizingDesktopSidebarRef.current) {
        return;
      }
      resizeDesktopSidebar(event.clientX);
    }

    function stopResize() {
      resizingDesktopSidebarRef.current = false;
      setResizingDesktopSidebar(false);
    }

    document.addEventListener("pointermove", handlePointerMove);
    document.addEventListener("pointerup", stopResize);
    document.addEventListener("pointercancel", stopResize);
    document.addEventListener("mousemove", handleMouseMove);
    document.addEventListener("mouseup", stopResize);

    return () => {
      document.body.style.cursor = previousCursor;
      document.body.style.userSelect = previousUserSelect;
      document.removeEventListener("pointermove", handlePointerMove);
      document.removeEventListener("pointerup", stopResize);
      document.removeEventListener("pointercancel", stopResize);
      document.removeEventListener("mousemove", handleMouseMove);
      document.removeEventListener("mouseup", stopResize);
    };
  }, [resizeDesktopSidebar, resizingDesktopSidebar]);

  function beginDesktopSidebarResize() {
    desktopSidebarWidthBeforeResizeRef.current = desktopSidebarWidth;
    resizingDesktopSidebarRef.current = true;
    setResizingDesktopSidebar(true);
  }

  function startDesktopSidebarResize(event: ReactPointerEvent<HTMLDivElement>) {
    if (event.button > 0) {
      return;
    }

    event.preventDefault();
    event.currentTarget.setPointerCapture?.(event.pointerId);
    beginDesktopSidebarResize();
  }

  function startDesktopSidebarMouseResize(event: ReactMouseEvent<HTMLDivElement>) {
    if (event.button > 0) {
      return;
    }

    event.preventDefault();
    beginDesktopSidebarResize();
  }

  function continueDesktopSidebarResize(event: ReactPointerEvent<HTMLDivElement>) {
    if (!resizingDesktopSidebarRef.current) {
      return;
    }

    resizeDesktopSidebar(event.clientX);
  }

  function continueDesktopSidebarMouseResize(event: ReactMouseEvent<HTMLDivElement>) {
    if (!resizingDesktopSidebarRef.current) {
      return;
    }

    resizeDesktopSidebar(event.clientX);
  }

  function stopDesktopSidebarResize() {
    resizingDesktopSidebarRef.current = false;
    setResizingDesktopSidebar(false);
  }

  function handleDesktopSidebarResizeKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    const resizeStep = event.shiftKey ? 40 : 16;

    switch (event.key) {
      case "ArrowLeft": {
        event.preventDefault();
        const nextWidth = desktopSidebarWidth - resizeStep;
        if (nextWidth < DESKTOP_SIDEBAR_COLLAPSE_WIDTH) {
          setDesktopSidebarCollapsed(true);
        } else {
          setDesktopSidebarWidth(clamp(nextWidth, DESKTOP_SIDEBAR_MIN_WIDTH, DESKTOP_SIDEBAR_MAX_WIDTH));
        }
        break;
      }
      case "ArrowRight":
        event.preventDefault();
        setDesktopSidebarCollapsed(false);
        setDesktopSidebarWidth((current) =>
          clamp(current + resizeStep, DESKTOP_SIDEBAR_MIN_WIDTH, DESKTOP_SIDEBAR_MAX_WIDTH)
        );
        break;
      case "Home":
        event.preventDefault();
        setDesktopSidebarWidth(DESKTOP_SIDEBAR_MIN_WIDTH);
        break;
      case "End":
        event.preventDefault();
        setDesktopSidebarWidth(DESKTOP_SIDEBAR_MAX_WIDTH);
        break;
      case "Escape":
        event.preventDefault();
        setDesktopSidebarCollapsed(true);
        break;
    }
  }

  return (
    <TooltipProvider delayDuration={250}>
      <div ref={shellRef} className="workspace-canvas relative flex h-screen min-h-[640px] flex-col overflow-hidden text-ink">
        <header className="window-chrome flex h-10 shrink-0 items-center">
          <div
            className="window-chrome-sidebar hidden h-full shrink-0 items-center md:flex"
            style={{ width: `${chromeSidebarWidth}px` }}
          >
            <div className="traffic-light-space h-full shrink-0" data-tauri-drag-region="" />
            <div className="flex h-full min-w-0 flex-1 items-center gap-1.5 pr-2">
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="window-chrome-action"
                    aria-label={desktopSidebarCollapsed ? "Show project sidebar" : "Hide project sidebar"}
                    onClick={() => setDesktopSidebarCollapsed((collapsed) => !collapsed)}
                  >
                    <SidebarIcon className="h-4 w-4" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>{desktopSidebarCollapsed ? "Show sidebar" : "Hide sidebar"}</TooltipContent>
              </Tooltip>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="window-chrome-action"
                aria-label="Back"
                disabled
              >
                <ArrowLeft className="h-4 w-4" />
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="window-chrome-action"
                aria-label="Forward"
                disabled
              >
                <ArrowRight className="h-4 w-4" />
              </Button>
              <div className="h-full min-w-0 flex-1" data-tauri-drag-region="" />
            </div>
          </div>

          <div className="flex h-full min-w-0 flex-1 items-center gap-3 px-3 md:px-4">
            <div className="flex min-w-0 items-center gap-2">
              <Sheet>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <SheetTrigger asChild>
                      <Button variant="ghost" size="icon" className="md:hidden" aria-label="Open sidebar">
                        <SidebarIcon className="h-4 w-4" />
                      </Button>
                    </SheetTrigger>
                  </TooltipTrigger>
                  <TooltipContent>Open sidebar</TooltipContent>
                </Tooltip>
                <SheetContent side="left" className="sidebar-surface w-[280px] border-border p-0">
                  <SheetHeader className="sr-only">
                    <SheetTitle>Projects and sessions</SheetTitle>
                  </SheetHeader>
                  <Sidebar state={workbench} />
                </SheetContent>
              </Sheet>

              <div className="window-session-title flex min-w-0 items-center gap-2" data-tauri-drag-region="">
                <span
                  aria-hidden="true"
                  className={cn("h-1.5 w-1.5 shrink-0 rounded-full", sessionStatusDotClass(activeStatus))}
                />
                <p className="type-label-md min-w-0 truncate text-ink">{activeSession?.title ?? "New session"}</p>
              </div>
            </div>

            <div className="h-full min-w-4 flex-1" data-tauri-drag-region="" />

            <div className="flex min-w-0 items-center gap-2">
              <div className="hidden min-w-0 items-center gap-2 sm:flex">
                <span className="type-code-sm max-w-[180px] truncate text-muted" data-tauri-drag-region="">
                  {runtimeModel}
                </span>
                <span className="window-status-pill type-label-sm text-muted" data-tauri-drag-region="">
                  {activeStatus.replace("_", " ")}
                </span>
              </div>
              {pendingApprovalCount > 0 || workbench.approvalInboxOpen ? (
                <Sheet open={workbench.approvalInboxOpen} onOpenChange={workbench.setApprovalInboxOpen}>
                  {pendingApprovalCount > 0 ? (
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <SheetTrigger asChild>
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            className="min-w-9 px-2"
                            aria-label={approvalInboxLabel}
                          >
                            <ShieldAlert className="h-4 w-4" />
                            <Badge variant="warning">{pendingApprovalCount}</Badge>
                          </Button>
                        </SheetTrigger>
                      </TooltipTrigger>
                      <TooltipContent>{approvalInboxLabel}</TooltipContent>
                    </Tooltip>
                  ) : null}
                  <SheetContent side="right" className="w-[min(520px,calc(100vw-24px))] p-0">
                    <SheetHeader className="sr-only">
                      <SheetTitle>{t("approvals.inbox.title")}</SheetTitle>
                      <SheetDescription>{t("approvals.inbox.description")}</SheetDescription>
                    </SheetHeader>
                    <ApprovalInbox />
                  </SheetContent>
                </Sheet>
              ) : null}
              <Sheet open={memoryOpen} onOpenChange={setMemoryOpen}>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      aria-label="Open memory"
                      onClick={() => setMemoryOpen(true)}
                    >
                      <Database className="h-4 w-4" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>Memory</TooltipContent>
                </Tooltip>
                <SheetContent side="right" className="w-[min(680px,calc(100vw-24px))] p-0">
                  <SheetHeader className="sr-only">
                    <SheetTitle>Memory</SheetTitle>
                    <SheetDescription>Project memory governance</SheetDescription>
                  </SheetHeader>
                  <MemoryInspector projectId={workbench.activeProjectId} />
                </SheetContent>
              </Sheet>
              <Sheet>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <SheetTrigger asChild>
                      <Button variant="ghost" size="icon" className="2xl:hidden" aria-label="Open inspector">
                        <PanelRight className="h-4 w-4" />
                      </Button>
                    </SheetTrigger>
                  </TooltipTrigger>
                  <TooltipContent>Open inspector</TooltipContent>
                </Tooltip>
                <SheetContent side="right" className="w-[min(320px,calc(100vw-24px))] p-0">
                  <SheetHeader className="sr-only">
                    <SheetTitle>Inspector</SheetTitle>
                  </SheetHeader>
                  <Inspector state={workbench} />
                </SheetContent>
              </Sheet>
            </div>
          </div>
        </header>

        <div className="workspace-body flex min-h-0 flex-1">
          {!desktopSidebarCollapsed ? (
            <aside
              aria-label="Projects and sessions"
              className="sidebar-surface relative hidden shrink-0 border-r border-border md:block"
              style={{ width: `${desktopSidebarWidth}px` }}
            >
              <Sidebar state={workbench} />
              <div
                aria-label="Resize project sidebar"
                aria-orientation="vertical"
                aria-valuemax={DESKTOP_SIDEBAR_MAX_WIDTH}
                aria-valuemin={DESKTOP_SIDEBAR_MIN_WIDTH}
                aria-valuenow={desktopSidebarWidth}
                className={cn(
                  "absolute -right-1 top-0 z-10 h-full w-2 cursor-col-resize touch-none outline-none",
                  "before:absolute before:left-1/2 before:top-0 before:h-full before:w-px before:-translate-x-1/2 before:bg-transparent before:transition-colors",
                  "hover:before:bg-border-strong focus-visible:before:bg-focus",
                  resizingDesktopSidebar && "before:bg-focus"
                )}
                role="separator"
                tabIndex={0}
                onKeyDown={handleDesktopSidebarResizeKeyDown}
                onMouseDown={startDesktopSidebarMouseResize}
                onMouseMove={continueDesktopSidebarMouseResize}
                onMouseUp={stopDesktopSidebarResize}
                onPointerDown={startDesktopSidebarResize}
                onPointerMove={continueDesktopSidebarResize}
                onPointerUp={stopDesktopSidebarResize}
                onPointerCancel={stopDesktopSidebarResize}
              />
            </aside>
          ) : null}

          <main className="flex min-w-0 flex-1 flex-col">
            <ChatView state={workbench} />
          </main>
        </div>

        <AgentThreadViewer
          agent={selectedAgent}
          view={workbench.selectedAgentView}
          tokenUsage={selectedAgentTokenUsage}
          onClose={workbench.closeAgentThread}
        />
      </div>
    </TooltipProvider>
  );
}

function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), max);
}

function sessionStatusDotClass(status: string) {
  switch (status) {
    case "running":
      return "bg-success motion-safe:animate-pulse";
    case "awaiting_approval":
      return "bg-warning";
    case "failed":
      return "bg-danger";
    default:
      return "bg-subtle";
  }
}

function findAgentNode(nodes: AgentNode[], threadId: string): AgentNode | null {
  for (const node of nodes) {
    if (node.threadId === threadId) {
      return node;
    }
    const child = findAgentNode(node.children, threadId);
    if (child) {
      return child;
    }
  }
  return null;
}
