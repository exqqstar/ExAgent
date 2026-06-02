import { useEffect, useState } from "react";
import { Plus, Save, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { useWorkbenchStore } from "@/stores/workbenchStore";
import type { SkillRootSettings } from "@/types";

export function SkillsSettingsPanel() {
  const runtimeSettings = useWorkbenchStore((state) => state.runtimeSettings);
  const saveRuntimeSettings = useWorkbenchStore((state) => state.saveRuntimeSettings);
  const [roots, setRoots] = useState<SkillRootSettings[]>(() => cloneRoots(runtimeSettings?.skill_roots ?? []));
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    setRoots(cloneRoots(runtimeSettings?.skill_roots ?? []));
  }, [runtimeSettings]);

  function addRoot() {
    setRoots((current) => [
      ...current,
      {
        id: `skill-${Date.now().toString(36)}`,
        name: "New skill root",
        enabled: false,
        path: "",
        scope: "user"
      }
    ]);
  }

  function updateRoot(id: string, patch: Partial<SkillRootSettings>) {
    setRoots((current) => current.map((root) => (root.id === id ? { ...root, ...patch } : root)));
  }

  async function save() {
    if (!runtimeSettings) {
      return;
    }
    setSaving(true);
    await saveRuntimeSettings({
      ...runtimeSettings,
      skill_roots: roots
    });
    setSaving(false);
  }

  return (
    <div className="space-y-5 pb-1">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
        <div>
          <h2 className="text-[22px] font-semibold text-ink">Skills</h2>
          <p className="mt-1 text-sm text-muted">
            Register skill roots that should be available in desktop sessions.
          </p>
        </div>
        <Button type="button" variant="secondary" onClick={addRoot}>
          <Plus className="h-4 w-4" />
          Add skill root
        </Button>
      </div>

      <div className="rounded-lg border border-border bg-surface-1">
        {roots.length === 0 ? (
          <p className="px-4 py-6 text-sm text-muted">No skill roots configured.</p>
        ) : (
          roots.map((root, index) => (
            <div key={root.id}>
              {index > 0 ? <Separator /> : null}
              <div className="grid gap-4 p-4">
                <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                  <label className="flex items-center gap-2 text-sm font-medium text-ink">
                    <input
                      type="checkbox"
                      checked={root.enabled}
                      onChange={(event) => updateRoot(root.id, { enabled: event.target.checked })}
                    />
                    Enabled
                  </label>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => setRoots((current) => current.filter((item) => item.id !== root.id))}
                  >
                    <Trash2 className="h-4 w-4" />
                    Remove
                  </Button>
                </div>

                <div className="grid gap-3 md:grid-cols-[1fr_160px]">
                  <label className="grid gap-2 text-sm font-medium text-muted">
                    Name
                    <Input value={root.name} onChange={(event) => updateRoot(root.id, { name: event.target.value })} />
                  </label>
                  <label className="grid gap-2 text-sm font-medium text-muted">
                    Scope
                    <Input value={root.scope} onChange={(event) => updateRoot(root.id, { scope: event.target.value })} />
                  </label>
                  <label className="grid gap-2 text-sm font-medium text-muted md:col-span-2">
                    Path
                    <Input
                      value={root.path}
                      placeholder="/Users/name/.codex/skills"
                      onChange={(event) => updateRoot(root.id, { path: event.target.value })}
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
          {saving ? "Saving" : "Save skills"}
        </Button>
      </div>
    </div>
  );
}

function cloneRoots(roots: SkillRootSettings[]) {
  return roots.map((root) => ({ ...root }));
}
