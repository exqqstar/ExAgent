import { useEffect, useState } from "react";
import { Archive, ExternalLink, RefreshCw, RotateCcw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";
import { SettingsPanel, SettingsPanelCard, SettingsPanelHeader } from "@/components/settings/SettingsPanelPrimitives";
import { exagentClient } from "@/api/exagentClient";
import { useWorkbenchStore } from "@/stores/workbenchStore";
import { useI18n } from "@/lib/i18n";
import type { ProjectRecord, SessionSummary } from "@/types";

type ArchivedProjectGroup = {
  project: ProjectRecord;
  sessions: SessionSummary[];
};

export function ArchivedConversationsPanel({ active }: { active: boolean }) {
  const { t } = useI18n();
  const unarchiveSession = useWorkbenchStore((state) => state.unarchiveSession);
  const openArchivedSession = useWorkbenchStore((state) => state.openArchivedSession);
  const [groups, setGroups] = useState<ArchivedProjectGroup[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!active) {
      return;
    }
    void loadArchivedConversations();
  }, [active]);

  async function loadArchivedConversations() {
    setLoading(true);
    setError(null);
    try {
      const projects = await exagentClient.listProjects();
      const nextGroups = await Promise.all(
        projects.map(async (project) => {
          const threads = await exagentClient.listThreads(project.id, true, null);
          return {
            project,
            sessions: threads
              .filter((thread) => thread.archived_at !== null)
              .map(exagentClient.threadRecordToSession)
          };
        })
      );
      setGroups(nextGroups.filter((group) => group.sessions.length > 0));
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setLoading(false);
    }
  }

  async function restore(projectId: string, sessionId: string) {
    await unarchiveSession(projectId, sessionId);
    await loadArchivedConversations();
  }

  async function open(projectId: string, sessionId: string) {
    await openArchivedSession(projectId, sessionId);
    await loadArchivedConversations();
  }

  return (
    <SettingsPanel>
      <SettingsPanelHeader
        title={t("settings.archive.title")}
        description={t("settings.archive.description")}
        action={
          <Button type="button" variant="secondary" onClick={() => void loadArchivedConversations()} disabled={loading}>
            <RefreshCw className="h-4 w-4" />
            {t("settings.archive.refresh")}
          </Button>
        }
      />

      <SettingsPanelCard>
        {loading ? <p className="type-body-md px-4 py-6 text-muted">{t("settings.archive.loading")}</p> : null}
        {error ? <p className="type-body-md px-4 py-6 text-danger">{error}</p> : null}
        {!loading && !error && groups.length === 0 ? (
          <div className="grid gap-2 px-4 py-8 text-center">
            <Archive className="mx-auto h-5 w-5 text-subtle" />
            <p className="type-title-md text-ink">{t("settings.archive.emptyTitle")}</p>
            <p className="type-body-md text-muted">{t("settings.archive.emptyDescription")}</p>
          </div>
        ) : null}
        {!loading && !error
          ? groups.map((group, groupIndex) => (
              <div key={group.project.id}>
                {groupIndex > 0 ? <Separator /> : null}
                <div className="grid gap-3 p-4">
                  <div className="min-w-0">
                    <h3 className="type-title-md truncate text-ink">{group.project.name}</h3>
                    <p className="type-body-sm truncate text-muted">{group.project.path}</p>
                  </div>
                  <div className="space-y-1">
                    {group.sessions.map((session) => (
                      <div
                        key={session.id}
                        className="flex min-w-0 items-center gap-3 rounded-md border border-border bg-surface-2 px-3 py-2"
                      >
                        <div className="min-w-0 flex-1">
                          <div className="flex min-w-0 items-center gap-2">
                            <p className="type-label-md truncate text-ink">{session.title}</p>
                            <Badge variant="neutral">{t("settings.archive.archived")}</Badge>
                          </div>
                          <p className="type-body-sm text-muted">{session.updatedAt}</p>
                        </div>
                        <Button type="button" variant="ghost" size="sm" onClick={() => void restore(group.project.id, session.id)}>
                          <RotateCcw className="h-4 w-4" />
                          {t("settings.archive.restore")}
                        </Button>
                        <Button type="button" variant="secondary" size="sm" onClick={() => void open(group.project.id, session.id)}>
                          <ExternalLink className="h-4 w-4" />
                          {t("settings.archive.open")}
                        </Button>
                      </div>
                    ))}
                  </div>
                </div>
              </div>
            ))
          : null}
      </SettingsPanelCard>
    </SettingsPanel>
  );
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}
