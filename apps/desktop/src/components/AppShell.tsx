import { useEffect } from "react";
import { PanelRight, SidebarIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent, SheetHeader, SheetTitle, SheetTrigger } from "@/components/ui/sheet";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { ChatView } from "@/components/ChatView";
import { Inspector } from "@/components/Inspector";
import { Sidebar } from "@/components/Sidebar";
import { loadWorkbench, useWorkbenchStore } from "@/stores/workbenchStore";

export function AppShell() {
  const workbench = useWorkbenchStore();
  const activeSession = workbench.sessions.find((session) => session.id === workbench.activeSessionId);

  useEffect(() => {
    if (workbench.loading && workbench.projects.length === 0) {
      void loadWorkbench();
    }
  }, [workbench.loading, workbench.projects.length]);

  return (
    <TooltipProvider delayDuration={250}>
      <div className="relative flex h-screen min-h-[640px] overflow-hidden bg-bg text-ink">
        <aside className="hidden w-[280px] shrink-0 border-r border-border bg-surface-1 md:block">
          <Sidebar state={workbench} />
        </aside>

        <main className="flex min-w-0 flex-1 flex-col 2xl:pr-[370px]">
          <header className="flex h-12 shrink-0 items-center justify-between border-b border-border bg-bg px-3">
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
                <SheetContent side="left" className="w-[280px] p-0">
                  <SheetHeader className="sr-only">
                    <SheetTitle>Projects and sessions</SheetTitle>
                  </SheetHeader>
                  <Sidebar state={workbench} />
                </SheetContent>
              </Sheet>

              <div className="min-w-0">
                <p className="truncate text-sm font-medium text-ink">{activeSession?.title ?? "ExAgent"}</p>
                <p className="truncate text-xs text-muted">{workbench.cwd}</p>
              </div>
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
              <SheetContent side="right" className="w-[min(340px,calc(100vw-24px))] p-0">
                <SheetHeader className="sr-only">
                  <SheetTitle>Inspector</SheetTitle>
                </SheetHeader>
                <Inspector state={workbench} />
              </SheetContent>
            </Sheet>
          </header>

          <ChatView state={workbench} />
        </main>

        <div className="pointer-events-none absolute bottom-5 right-5 top-16 z-20 hidden w-[330px] 2xl:block">
          <aside
            className="pointer-events-auto h-full overflow-hidden rounded-lg border border-border bg-surface-1 shadow-panel"
            aria-label="Inspector"
          >
            <Inspector state={workbench} />
          </aside>
        </div>
      </div>
    </TooltipProvider>
  );
}
