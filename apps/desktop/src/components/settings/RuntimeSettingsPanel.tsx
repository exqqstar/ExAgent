import { useEffect, useState } from "react";
import { Save } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useWorkbenchStore } from "@/stores/workbenchStore";
import type { ThinkingMode } from "@/types";
import { cn } from "@/lib/utils";

const thinkingModes: Array<{ value: ThinkingMode; label: string }> = [
  { value: "auto", label: "Auto" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" }
];

export function RuntimeSettingsPanel() {
  const runtimeSettings = useWorkbenchStore((state) => state.runtimeSettings);
  const saveRuntimeSettings = useWorkbenchStore((state) => state.saveRuntimeSettings);
  const [defaultModel, setDefaultModel] = useState(runtimeSettings?.default_model ?? "");
  const [thinkingMode, setThinkingMode] = useState<ThinkingMode>(runtimeSettings?.default_thinking_mode ?? "auto");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    setDefaultModel(runtimeSettings?.default_model ?? "");
    setThinkingMode(runtimeSettings?.default_thinking_mode ?? "auto");
  }, [runtimeSettings]);

  async function save() {
    if (!runtimeSettings) {
      return;
    }
    setSaving(true);
    await saveRuntimeSettings({
      ...runtimeSettings,
      default_model: defaultModel,
      default_thinking_mode: thinkingMode
    });
    setSaving(false);
  }

  return (
    <div className="space-y-5 pb-1">
      <div>
        <h2 className="text-[22px] font-semibold text-ink">Runtime</h2>
        <p className="mt-1 text-sm text-muted">
          Set the defaults used by new chat turns.
        </p>
      </div>

      <div className="grid gap-4 rounded-lg border border-border bg-surface-1 p-4">
        <label className="grid gap-2 text-sm font-medium text-muted">
          Default model
          <Input
            className="h-10 text-base"
            value={defaultModel}
            placeholder="gpt-4.1"
            onChange={(event) => setDefaultModel(event.target.value)}
          />
        </label>

        <div className="grid gap-2">
          <span className="text-sm font-medium text-muted">Default thinking mode</span>
          <div className="grid grid-cols-2 gap-1 sm:grid-cols-4" role="group" aria-label="Default thinking mode">
            {thinkingModes.map((mode) => {
              const selected = thinkingMode === mode.value;
              return (
                <button
                  key={mode.value}
                  type="button"
                  aria-pressed={selected}
                  className={cn(
                    "h-9 rounded-md border px-3 text-sm font-medium transition-colors",
                    selected
                      ? "border-border-strong bg-surface-3 text-ink"
                      : "border-border bg-surface-2 text-muted hover:bg-surface-3 hover:text-ink"
                  )}
                  onClick={() => setThinkingMode(mode.value)}
                >
                  {mode.label}
                </button>
              );
            })}
          </div>
        </div>

        <div className="flex justify-end">
          <Button type="button" disabled={!runtimeSettings || saving} onClick={save}>
            <Save className="h-4 w-4" />
            {saving ? "Saving" : "Save runtime"}
          </Button>
        </div>
      </div>
    </div>
  );
}
