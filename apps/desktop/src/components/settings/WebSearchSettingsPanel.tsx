import { useEffect, useState } from "react";
import { Save } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  SettingsPanel,
  SettingsPanelCard,
  SettingsPanelHeader,
} from "@/components/settings/SettingsPanelPrimitives";
import { useWorkbenchStore } from "@/stores/workbenchStore";
import type { WebSearchSettings } from "@/types";

const defaultWebSearchSettings: WebSearchSettings = {
  enabled: false,
  provider: "brave",
  has_api_key: false,
  api_key: null,
  clear_api_key: false,
};

export function WebSearchSettingsPanel() {
  const runtimeSettings = useWorkbenchStore((state) => state.runtimeSettings);
  const saveRuntimeSettings = useWorkbenchStore(
    (state) => state.saveRuntimeSettings,
  );
  const [settings, setSettings] = useState<WebSearchSettings>(
    () => runtimeSettings?.web_search ?? defaultWebSearchSettings,
  );
  const [apiKey, setApiKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setSettings(runtimeSettings?.web_search ?? defaultWebSearchSettings);
    setApiKey("");
    setError(null);
  }, [runtimeSettings]);

  async function save() {
    if (!runtimeSettings) {
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const nextWebSearch: WebSearchSettings = {
        ...settings,
        provider: "brave",
        api_key: apiKey.trim() ? apiKey.trim() : null,
        clear_api_key: settings.clear_api_key ?? false,
      };
      await saveRuntimeSettings({
        ...runtimeSettings,
        web_search: nextWebSearch,
      });
      setApiKey("");
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setSaving(false);
    }
  }

  const hasUsableKey =
    apiKey.trim().length > 0 ||
    (settings.has_api_key && settings.clear_api_key !== true);
  const saveDisabled = saving || !runtimeSettings || (settings.enabled && !hasUsableKey);

  return (
    <SettingsPanel>
      <SettingsPanelHeader
        title="Web search"
        description="Expose a Brave-backed web_search tool to new runtime sessions."
        action={
          <Badge variant={settings.enabled ? "success" : "neutral"}>
            {settings.enabled ? "Enabled" : "Disabled"}
          </Badge>
        }
      />

      <SettingsPanelCard>
        <div className="grid gap-4 p-4">
          <label className="type-label-md flex items-center gap-2 text-ink">
            <input
              type="checkbox"
              checked={settings.enabled}
              onChange={(event) =>
                setSettings((current) => ({
                  ...current,
                  enabled: event.target.checked,
                }))
              }
            />
            Enable web_search
          </label>

          <label className="type-label-md grid gap-2 text-muted">
            Provider
            <Input value="Brave" disabled />
          </label>

          <label className="type-label-md grid gap-2 text-muted">
            Brave Search API key
            <Input
              value={apiKey}
              type="password"
              autoComplete="off"
              placeholder={settings.has_api_key ? "Saved key" : "Required when enabled"}
              onChange={(event) => {
                setApiKey(event.target.value);
                setSettings((current) => ({
                  ...current,
                  clear_api_key: false,
                }));
              }}
            />
          </label>

          {settings.has_api_key ? (
            <label className="type-label-md flex items-center gap-2 text-muted">
              <input
                type="checkbox"
                checked={settings.clear_api_key ?? false}
                onChange={(event) =>
                  setSettings((current) => ({
                    ...current,
                    clear_api_key: event.target.checked,
                  }))
                }
              />
              Clear saved key
            </label>
          ) : null}

          {error ? <p className="type-body-md text-danger">{error}</p> : null}

          <div>
            <Button type="button" onClick={save} disabled={saveDisabled}>
              <Save className="h-4 w-4" />
              {saving ? "Saving..." : "Save web search"}
            </Button>
          </div>
        </div>
      </SettingsPanelCard>
    </SettingsPanel>
  );
}

function errorMessage(reason: unknown): string {
  if (reason instanceof Error) {
    return reason.message;
  }
  return typeof reason === "string" ? reason : "Failed to save web search settings";
}
