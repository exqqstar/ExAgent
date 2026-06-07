import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent
} from "react";
import { PanelRight, SidebarIcon } from "lucide-react";
import { AgentThreadViewer } from "@/components/AgentThreadViewer";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent, SheetHeader, SheetTitle, SheetTrigger } from "@/components/ui/sheet";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { ChatView } from "@/components/ChatView";
import { Inspector } from "@/components/Inspector";
import { Sidebar } from "@/components/Sidebar";
import { loadWorkbench, useWorkbenchStore } from "@/stores/workbenchStore";
import type { AgentNode } from "@/types";
import { cn } from "@/lib/utils";

const DESKTOP_SIDEBAR_DEFAULT_WIDTH = 280;
const DESKTOP_SIDEBAR_MIN_WIDTH = 240;
const DESKTOP_SIDEBAR_MAX_WIDTH = 420;
const DESKTOP_SIDEBAR_COLLAPSE_WIDTH = 220;

export function AppShell() {
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
      <div ref={shellRef} className="workspace-canvas relative flex h-screen min-h-[640px] overflow-hidden text-ink">
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
          <header className="topbar-surface flex h-12 shrink-0 items-center justify-between border-b border-border px-3">
            <div className="flex min-w-0 items-center gap-2">
              {desktopSidebarCollapsed ? (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="hidden md:inline-flex"
                      aria-label="Show project sidebar"
                      onClick={() => setDesktopSidebarCollapsed(false)}
                    >
                      <SidebarIcon className="h-4 w-4" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>Show sidebar</TooltipContent>
                </Tooltip>
              ) : null}
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
                <SheetContent
                  side="left"
                  className="sidebar-surface w-[280px] border-border p-0"
                >
                  <SheetHeader className="sr-only">
                    <SheetTitle>Projects and sessions</SheetTitle>
                  </SheetHeader>
                  <Sidebar state={workbench} />
                </SheetContent>
              </Sheet>

              <div className="min-w-0">
                <div className="flex min-w-0 items-center gap-2">
                  <span
                    aria-hidden="true"
                    className={cn("h-1.5 w-1.5 shrink-0 rounded-full", sessionStatusDotClass(activeStatus))}
                  />
                  <p className="type-label-md truncate text-ink">{activeSession?.title ?? "ExAgent"}</p>
                </div>
              </div>
            </div>

            <div className="flex min-w-0 items-center gap-2">
              <div className="hidden min-w-0 items-center gap-2 sm:flex">
                <span className="type-code-sm max-w-[180px] truncate text-muted">{runtimeModel}</span>
                <span className="type-label-sm rounded border border-border bg-surface-2 px-1.5 py-1 text-muted">
                  {activeStatus.replace("_", " ")}
                </span>
              </div>
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
          </header>

          <ChatView state={workbench} />
        </main>

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
