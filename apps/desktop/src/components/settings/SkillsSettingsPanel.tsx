import { useCallback, useEffect, useMemo, useState } from "react";
import { AlertTriangle, FolderGit2, Globe2, Layers3, Plus, RefreshCw, Save, Trash2 } from "lucide-react";
import { scanSkillCatalog } from "@/api/exagentClient";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { SettingsPanel, SettingsPanelCard, SettingsPanelHeader } from "@/components/settings/SettingsPanelPrimitives";
import { cn } from "@/lib/utils";
import { useWorkbenchStore } from "@/stores/workbenchStore";
import type { SkillCatalogItemView, SkillCatalogScanResponse, SkillRootSettings, SkillSourceView } from "@/types";

type BadgeVariant = "neutral" | "success" | "info" | "warning" | "danger" | "primary";

export function SkillsSettingsPanel() {
  const runtimeSettings = useWorkbenchStore((state) => state.runtimeSettings);
  const saveRuntimeSettings = useWorkbenchStore((state) => state.saveRuntimeSettings);
  const projects = useWorkbenchStore((state) => state.projects);
  const activeProjectId = useWorkbenchStore((state) => state.activeProjectId);
  const cwd = useWorkbenchStore((state) => state.cwd);
  const [roots, setRoots] = useState<SkillRootSettings[]>(() => cloneRoots(runtimeSettings?.skill_roots ?? []));
  const [scan, setScan] = useState<SkillCatalogScanResponse | null>(null);
  const [scanLoading, setScanLoading] = useState(false);
  const [scanError, setScanError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const workspaceRoot = useMemo(() => {
    const activeProject = projects.find((project) => project.id === activeProjectId);
    if (activeProject?.path) {
      return activeProject.path;
    }
    return cwd && cwd !== "No project selected" ? cwd : null;
  }, [activeProjectId, cwd, projects]);

  const refreshCatalog = useCallback(async () => {
    setScanLoading(true);
    setScanError(null);
    try {
      setScan(await scanSkillCatalog(workspaceRoot));
    } catch (error) {
      setScanError(error instanceof Error ? error.message : String(error));
    } finally {
      setScanLoading(false);
    }
  }, [workspaceRoot]);

  useEffect(() => {
    setRoots(cloneRoots(runtimeSettings?.skill_roots ?? []));
  }, [runtimeSettings]);

  useEffect(() => {
    void refreshCatalog();
  }, [refreshCatalog, runtimeSettings]);

  const sourcesById = useMemo(() => {
    const sources = new Map<string, SkillSourceView>();
    scan?.sources.forEach((source) => sources.set(source.id, source));
    return sources;
  }, [scan]);
  const projectSource = scan?.sources.find((source) => source.scope === "project") ?? null;
  const activeSkills = scan?.skills.filter((skill) => skill.status !== "shadowed").length ?? 0;
  const implicitSkills = scan?.skills.filter((skill) => skill.effective_implicit).length ?? 0;
  const explicitOnlySkills = scan?.skills.filter((skill) => skill.status === "explicit_only").length ?? 0;
  const warningCount = scan?.warnings.length ?? 0;

  function addRoot() {
    setRoots((current) => [
      ...current,
      {
        id: `skill-${Date.now().toString(36)}`,
        name: "Global skills",
        enabled: true,
        path: "",
        scope: "global"
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
    try {
      await saveRuntimeSettings({
        ...runtimeSettings,
        skill_roots: roots
      });
      await refreshCatalog();
    } finally {
      setSaving(false);
    }
  }

  return (
    <SettingsPanel>
      <SettingsPanelHeader
        title="Skills"
        description="Control global skill roots and review the effective project catalog."
        action={
          <Button type="button" variant="secondary" onClick={addRoot}>
            <Plus className="h-4 w-4" />
            Add global root
          </Button>
        }
      />

      <SettingsPanelCard className="grid gap-0 overflow-hidden sm:grid-cols-3">
        <SummaryStat label="Skills enabled" value={activeSkills.toString()} />
        <SummaryStat label="Implicit invocation" value={implicitSkills.toString()} detail={`${explicitOnlySkills} explicit-only`} />
        <SummaryStat
          label="Catalog warnings"
          value={warningCount.toString()}
          detail={scanLoading ? "Scanning" : scanError ? "Scan failed" : "Current scan"}
          tone={scanError || warningCount > 0 ? "warning" : "neutral"}
        />
      </SettingsPanelCard>

      <SettingsPanelCard>
        <SectionHeader title="Sources" detail={`${roots.length + (projectSource ? 1 : 0)} configured`} />
        {projectSource ? <ProjectSourceRow source={projectSource} /> : null}
        {projectSource && roots.length > 0 ? <Separator /> : null}
        {roots.length === 0 ? (
          <p className="type-body-md px-4 py-6 text-muted">No global roots configured.</p>
        ) : (
          roots.map((root, index) => {
            const source = sourcesById.get(sourceIdForRoot(root, index));
            return (
              <RootSourceRow
                key={root.id || index}
                root={root}
                source={source}
                onRemove={() => setRoots((current) => current.filter((item) => item.id !== root.id))}
                onUpdate={(patch) => updateRoot(root.id, patch)}
              />
            );
          })
        )}
        <Separator />
        <div className="flex justify-end p-4">
          <Button type="button" disabled={!runtimeSettings || saving} onClick={save}>
            <Save className="h-4 w-4" />
            {saving ? "Saving" : "Save sources"}
          </Button>
        </div>
      </SettingsPanelCard>

      <SettingsPanelCard>
        <div className="flex flex-col gap-3 p-4 sm:flex-row sm:items-start sm:justify-between">
          <div>
            <h3 className="type-title-md text-ink">Catalog</h3>
            <p className="type-body-sm mt-1 text-muted">{scan?.skills.length ?? 0} skills found</p>
          </div>
          <Button type="button" variant="outline" onClick={refreshCatalog} disabled={scanLoading}>
            <RefreshCw className={cn("h-4 w-4", scanLoading ? "animate-spin" : null)} />
            Rescan
          </Button>
        </div>
        <Separator />
        {scanError ? (
          <div className="flex items-start gap-3 p-4 text-warning">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
            <p className="type-body-md min-w-0 break-words">{scanError}</p>
          </div>
        ) : scanLoading && !scan ? (
          <p className="type-body-md px-4 py-6 text-muted">Scanning catalog.</p>
        ) : scan?.skills.length ? (
          <div>
            {scan.skills.map((skill, index) => (
              <SkillCatalogRow key={`${skill.source_id}:${skill.name}:${skill.path}`} skill={skill} first={index === 0} />
            ))}
          </div>
        ) : (
          <p className="type-body-md px-4 py-6 text-muted">No skills found.</p>
        )}
        {scan?.warnings.length ? (
          <>
            <Separator />
            <div className="grid gap-3 p-4">
              <div className="flex items-center gap-2 text-warning">
                <AlertTriangle className="h-4 w-4" />
                <h4 className="type-label-md">Warnings</h4>
              </div>
              {scan.warnings.map((warning, index) => (
                <div key={`${warning.kind}:${warning.scope}:${warning.name}:${index}`} className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <Badge variant="warning">{warning.kind.replaceAll("_", " ")}</Badge>
                    <span className="type-label-md text-ink">{warning.name}</span>
                    <span className="type-body-sm text-muted">{warning.scope}</span>
                  </div>
                  <p className="type-body-sm mt-1 break-all text-muted">{warning.paths.join(" -> ")}</p>
                </div>
              ))}
            </div>
          </>
        ) : null}
      </SettingsPanelCard>
    </SettingsPanel>
  );
}

function SummaryStat({
  detail,
  label,
  tone = "neutral",
  value
}: {
  detail?: string;
  label: string;
  tone?: "neutral" | "warning";
  value: string;
}) {
  return (
    <div className="min-w-0 border-b border-border p-4 last:border-b-0 sm:border-b-0 sm:border-r sm:last:border-r-0">
      <p className="type-label-md text-muted">{label}</p>
      <p className={cn("type-title-lg mt-1", tone === "warning" ? "text-warning" : "text-ink")}>{value}</p>
      {detail ? <p className="type-body-sm mt-1 break-words text-muted">{detail}</p> : null}
    </div>
  );
}

function SectionHeader({ detail, title }: { detail: string; title: string }) {
  return (
    <>
      <div className="flex items-center justify-between gap-3 p-4">
        <h3 className="type-title-md text-ink">{title}</h3>
        <span className="type-body-sm text-muted">{detail}</span>
      </div>
      <Separator />
    </>
  );
}

function ProjectSourceRow({ source }: { source: SkillSourceView }) {
  return (
    <div className="grid gap-3 p-4">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <FolderGit2 className="h-4 w-4 text-muted" />
            <span className="type-label-md text-ink">{source.name}</span>
            <Badge variant="primary">project</Badge>
            <SourceStatusBadge status={source.status} />
          </div>
          <p className="type-body-sm mt-1 break-all text-muted">{source.path}</p>
        </div>
        <SourceCounts source={source} />
      </div>
    </div>
  );
}

function RootSourceRow({
  onRemove,
  onUpdate,
  root,
  source
}: {
  onRemove: () => void;
  onUpdate: (patch: Partial<SkillRootSettings>) => void;
  root: SkillRootSettings;
  source?: SkillSourceView;
}) {
  const toggleEnabled = isRuntimeRootScope(root.scope);

  return (
    <div>
      <div className="grid gap-4 p-4">
        <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <Globe2 className="h-4 w-4 text-muted" />
              <span className="type-label-md text-ink">{root.name || source?.name || "Global skills"}</span>
              <Badge variant="neutral">{root.scope || source?.scope || "global"}</Badge>
              {source ? <SourceStatusBadge status={source.status} /> : <Badge variant="neutral">unsaved</Badge>}
            </div>
            {source ? <SourceCounts className="mt-2 sm:hidden" source={source} /> : null}
          </div>
          <div className="flex shrink-0 items-center gap-3">
            {toggleEnabled ? (
              <label className="type-label-md flex items-center gap-2 text-muted">
                <input
                  type="checkbox"
                  checked={root.enabled}
                  onChange={(event) => onUpdate({ enabled: event.target.checked })}
                />
                Enabled
              </label>
            ) : null}
            <Button type="button" variant="ghost" size="sm" onClick={onRemove}>
              <Trash2 className="h-4 w-4" />
              Remove
            </Button>
          </div>
        </div>

        <div className="grid gap-3 md:grid-cols-[minmax(0,1fr)_minmax(0,2fr)]">
          <label className="type-label-md grid gap-2 text-muted">
            Name
            <Input value={root.name} onChange={(event) => onUpdate({ name: event.target.value })} />
          </label>
          <label className="type-label-md grid gap-2 text-muted">
            Path
            <Input
              value={root.path}
              placeholder="/Users/name/.codex/skills"
              onChange={(event) => onUpdate({ path: event.target.value })}
            />
          </label>
        </div>
      </div>
      <Separator />
    </div>
  );
}

function SourceCounts({ className, source }: { className?: string; source: SkillSourceView }) {
  return (
    <div className={cn("type-body-sm flex shrink-0 flex-wrap gap-3 text-muted", className)}>
      <span>{source.skill_count} skills</span>
      <span>{source.warning_count} warnings</span>
    </div>
  );
}

function SkillCatalogRow({ first, skill }: { first: boolean; skill: SkillCatalogItemView }) {
  return (
    <div>
      {!first ? <Separator /> : null}
      <div className="grid gap-2 p-4">
        <div className="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <Layers3 className="h-4 w-4 text-muted" />
              <span className="type-label-md text-ink">{skill.name}</span>
              <Badge variant={scopeBadgeVariant(skill.scope)}>{skill.scope}</Badge>
              <SkillStatusBadge skill={skill} />
            </div>
            {skill.description ? <p className="type-body-md mt-1 break-words text-muted">{skill.description}</p> : null}
          </div>
        </div>
        <p className="type-body-sm break-all text-muted">{skill.path}</p>
      </div>
    </div>
  );
}

function SourceStatusBadge({ status }: { status: string }) {
  return <Badge variant={sourceStatusVariant(status)}>{statusLabel(status)}</Badge>;
}

function SkillStatusBadge({ skill }: { skill: SkillCatalogItemView }) {
  const label = skill.status === "active" && skill.effective_implicit ? "implicit" : statusLabel(skill.status);
  return <Badge variant={skillStatusVariant(skill.status)}>{label}</Badge>;
}

function cloneRoots(roots: SkillRootSettings[]) {
  return roots.map((root) => ({ ...root }));
}

function isRuntimeRootScope(scope: string) {
  return ["global", "user"].includes(scope.trim().toLowerCase());
}

function sourceIdForRoot(root: SkillRootSettings, index: number) {
  return root.id.trim() || `skill-root-${index}`;
}

function sourceStatusVariant(status: string): BadgeVariant {
  if (status === "ready") {
    return "success";
  }
  if (status === "missing") {
    return "warning";
  }
  return "neutral";
}

function skillStatusVariant(status: string): BadgeVariant {
  if (status === "active") {
    return "success";
  }
  if (status === "explicit_only") {
    return "info";
  }
  if (status === "shadowed") {
    return "warning";
  }
  return "neutral";
}

function scopeBadgeVariant(scope: string): BadgeVariant {
  return scope === "project" ? "primary" : "neutral";
}

function statusLabel(status: string) {
  return status.replaceAll("_", " ");
}
