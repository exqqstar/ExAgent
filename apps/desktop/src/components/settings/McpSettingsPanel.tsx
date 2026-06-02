import { useEffect, useState } from "react";
import { Plus, Save, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { useWorkbenchStore } from "@/stores/workbenchStore";
import type { McpServerSettings } from "@/types";

export function McpSettingsPanel() {
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
        name: "New MCP server",
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
    <div className="space-y-5 pb-1">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
        <div>
          <h2 className="text-[22px] font-semibold text-ink">MCP</h2>
          <p className="mt-1 text-sm text-muted">
            Configure local MCP servers exposed to the desktop runtime.
          </p>
        </div>
        <Button type="button" variant="secondary" onClick={addServer}>
          <Plus className="h-4 w-4" />
          Add MCP server
        </Button>
      </div>

      <div className="rounded-lg border border-border bg-surface-1">
        {servers.length === 0 ? (
          <p className="px-4 py-6 text-sm text-muted">No MCP servers configured.</p>
        ) : (
          servers.map((server, index) => (
            <div key={server.id}>
              {index > 0 ? <Separator /> : null}
              <div className="grid gap-4 p-4">
                <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                  <label className="flex items-center gap-2 text-sm font-medium text-ink">
                    <input
                      type="checkbox"
                      checked={server.enabled}
                      onChange={(event) => updateServer(server.id, { enabled: event.target.checked })}
                    />
                    Enabled
                  </label>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => setServers((current) => current.filter((item) => item.id !== server.id))}
                  >
                    <Trash2 className="h-4 w-4" />
                    Remove
                  </Button>
                </div>

                <div className="grid gap-3 md:grid-cols-2">
                  <label className="grid gap-2 text-sm font-medium text-muted">
                    Name
                    <Input value={server.name} onChange={(event) => updateServer(server.id, { name: event.target.value })} />
                  </label>
                  <label className="grid gap-2 text-sm font-medium text-muted">
                    Command
                    <Input value={server.command} onChange={(event) => updateServer(server.id, { command: event.target.value })} />
                  </label>
                  <label className="grid gap-2 text-sm font-medium text-muted md:col-span-2">
                    Arguments
                    <Input
                      value={server.args.join(" ")}
                      placeholder="--yes @modelcontextprotocol/server-filesystem /tmp"
                      onChange={(event) => updateServer(server.id, { args: splitWords(event.target.value) })}
                    />
                  </label>
                  <label className="grid gap-2 text-sm font-medium text-muted">
                    Working directory
                    <Input
                      value={server.working_directory ?? ""}
                      placeholder="/Volumes/EXEXEX/ExAgent"
                      onChange={(event) =>
                        updateServer(server.id, { working_directory: event.target.value.trim() || null })
                      }
                    />
                  </label>
                  <label className="grid gap-2 text-sm font-medium text-muted">
                    Environment
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
      </div>

      <div className="flex justify-end">
        <Button type="button" disabled={!runtimeSettings || saving} onClick={save}>
          <Save className="h-4 w-4" />
          {saving ? "Saving" : "Save MCP"}
        </Button>
      </div>
    </div>
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
