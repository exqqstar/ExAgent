import { useEffect, useState } from "react";
import { Plus, Save, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { SettingsPanel, SettingsPanelCard, SettingsPanelHeader } from "@/components/settings/SettingsPanelPrimitives";
import { useWorkbenchStore } from "@/stores/workbenchStore";
import { useI18n } from "@/lib/i18n";
import type { McpServerSettings } from "@/types";

export function McpSettingsPanel() {
  const { t } = useI18n();
  const runtimeSettings = useWorkbenchStore((state) => state.runtimeSettings);
  const saveRuntimeSettings = useWorkbenchStore((state) => state.saveRuntimeSettings);
  const [servers, setServers] = useState<McpServerSettings[]>(() => cloneServers(runtimeSettings?.mcp_servers ?? []));
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    setServers(cloneServers(runtimeSettings?.mcp_servers ?? []));
  }, [runtimeSettings]);

  function addServer() {
    setServers((current) => [
      ...current,
      {
        id: `mcp-${Date.now().toString(36)}`,
        name: t("settings.mcp.newName"),
        enabled: false,
        command: "npx",
        args: [],
        env: [],
        working_directory: null
      }
    ]);
  }

  function updateServer(id: string, patch: Partial<McpServerSettings>) {
    setServers((current) => current.map((server) => (server.id === id ? { ...server, ...patch } : server)));
  }

  async function save() {
    if (!runtimeSettings) {
      return;
    }
    setSaving(true);
    await saveRuntimeSettings({
      ...runtimeSettings,
      mcp_servers: servers
    });
    setSaving(false);
  }

  return (
    <SettingsPanel>
      <SettingsPanelHeader
        title={t("settings.mcp.title")}
        description={t("settings.mcp.description")}
        action={
          <Button type="button" variant="secondary" onClick={addServer}>
            <Plus className="h-4 w-4" />
            {t("settings.mcp.add")}
          </Button>
        }
      />

      <SettingsPanelCard>
        {servers.length === 0 ? (
          <p className="type-body-md px-4 py-6 text-muted">{t("settings.mcp.empty")}</p>
        ) : (
          servers.map((server, index) => (
            <div key={server.id}>
              {index > 0 ? <Separator /> : null}
              <div className="grid gap-4 p-4">
                <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                  <label className="type-label-md flex items-center gap-2 text-ink">
                    <input
                      type="checkbox"
                      checked={server.enabled}
                      onChange={(event) => updateServer(server.id, { enabled: event.target.checked })}
                    />
                    {t("common.enabled")}
                  </label>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => setServers((current) => current.filter((item) => item.id !== server.id))}
                  >
                    <Trash2 className="h-4 w-4" />
                    {t("common.remove")}
                  </Button>
                </div>

                <div className="grid gap-3 md:grid-cols-2">
                  <label className="type-label-md grid gap-2 text-muted">
                    {t("common.name")}
                    <Input value={server.name} onChange={(event) => updateServer(server.id, { name: event.target.value })} />
                  </label>
                  <label className="type-label-md grid gap-2 text-muted">
                    {t("common.command")}
                    <Input value={server.command} onChange={(event) => updateServer(server.id, { command: event.target.value })} />
                  </label>
                  <label className="type-label-md grid gap-2 text-muted md:col-span-2">
                    {t("common.arguments")}
                    <Input
                      value={server.args.join(" ")}
                      placeholder="--yes @modelcontextprotocol/server-filesystem /tmp"
                      onChange={(event) => updateServer(server.id, { args: splitWords(event.target.value) })}
                    />
                  </label>
                  <label className="type-label-md grid gap-2 text-muted">
                    {t("common.workingDirectory")}
                    <Input
                      value={server.working_directory ?? ""}
                      placeholder="/Volumes/EXEXEX/ExAgent"
                      onChange={(event) =>
                        updateServer(server.id, { working_directory: event.target.value.trim() || null })
                      }
                    />
                  </label>
                  <label className="type-label-md grid gap-2 text-muted">
                    {t("common.environment")}
                    <Input
                      value={envToText(server.env)}
                      placeholder="TOKEN=... LOG_LEVEL=debug"
                      onChange={(event) => updateServer(server.id, { env: textToEnv(event.target.value) })}
                    />
                  </label>
                </div>
              </div>
            </div>
          ))
        )}
      </SettingsPanelCard>

      <div className="flex justify-end">
        <Button type="button" disabled={!runtimeSettings || saving} onClick={save}>
          <Save className="h-4 w-4" />
          {saving ? t("common.saving") : t("settings.mcp.save")}
        </Button>
      </div>
    </SettingsPanel>
  );
}

function cloneServers(servers: McpServerSettings[]) {
  return servers.map((server) => ({
    ...server,
    args: [...server.args],
    env: server.env.map(([key, value]) => [key, value] as [string, string])
  }));
}

function splitWords(value: string) {
  return value.split(/\s+/).map((item) => item.trim()).filter(Boolean);
}

function envToText(env: [string, string][]) {
  return env.map(([key, value]) => `${key}=${value}`).join(" ");
}

function textToEnv(value: string): [string, string][] {
  return splitWords(value)
    .map((entry) => {
      const [key, ...rest] = entry.split("=");
      return [key.trim(), rest.join("=")] as [string, string];
    })
    .filter(([key]) => key.length > 0);
}
