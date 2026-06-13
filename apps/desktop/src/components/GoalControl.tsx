import { useEffect, useState } from "react";
import type { LucideIcon } from "lucide-react";
import { Check, Gauge, Pause, Pencil, Play, ShieldCheck, Target, Trash2, X, Zap } from "lucide-react";
import {
  clearThreadGoal,
  closeThreadGoalEditor,
  saveThreadGoal,
  setThreadGoalStatus
} from "@/stores/workbenchStore";
import type { getWorkbenchState } from "@/stores/workbenchStore";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import type { DraftThreadGoal, ThreadGoal, ThreadGoalMode, ThreadGoalStatus } from "@/types";

type WorkbenchState = ReturnType<typeof getWorkbenchState>;
type GoalControlVariant = "dock" | "hero";

export function GoalControl({
  state,
  variant = "dock"
}: {
  state: WorkbenchState;
  variant?: GoalControlVariant;
}) {
  const goal = state.currentGoal;
  const draftGoal = state.draftGoal;
  const [editing, setEditing] = useState(false);
  const [objective, setObjective] = useState("");
  const [tokenBudget, setTokenBudget] = useState("");
  const [mode, setMode] = useState<ThreadGoalMode>("standard");
  const editorObjective = goal?.objective ?? draftGoal?.objective ?? state.composerValue.trim();
  const editorTokenBudget = goal?.token_budget ?? draftGoal?.token_budget ?? null;
  const editorMode = goal ? state.currentGoalMode : draftGoal?.mode ?? state.currentGoalMode;

  useEffect(() => {
    if (state.goalEditorOpen) {
      setObjective(editorObjective);
      setTokenBudget(formatTokenBudget(editorTokenBudget));
      setMode(editorMode);
      setEditing(true);
    }
  }, [editorMode, editorObjective, editorTokenBudget, state.goalEditorOpen]);

  useEffect(() => {
    if (state.goalEditorOpen) {
      return;
    }
    if (goal) {
      setObjective(goal.objective);
      setTokenBudget(formatTokenBudget(goal.token_budget ?? null));
      setMode(state.currentGoalMode);
      setEditing(false);
      return;
    }
    if (draftGoal) {
      setObjective(draftGoal.objective);
      setTokenBudget(formatTokenBudget(draftGoal.token_budget));
      setMode(draftGoal.mode);
      setEditing(false);
    }
  }, [
    draftGoal?.mode,
    draftGoal?.objective,
    draftGoal?.token_budget,
    goal?.goal_id,
    goal?.objective,
    goal?.token_budget,
    state.currentGoalMode,
    state.goalEditorOpen
  ]);

  if (!state.activeProjectId || (!state.activeSessionId && !draftGoal && !state.goalEditorOpen && !editing)) {
    return null;
  }

  const openEditor = () => {
    setObjective(editorObjective);
    setTokenBudget(formatTokenBudget(editorTokenBudget));
    setMode(editorMode);
    setEditing(true);
  };

  if (!goal && !draftGoal && !editing) {
    return (
      <div className={variant === "hero" ? "mt-3 flex justify-start" : "mb-2 flex justify-start"}>
        <Button type="button" variant="secondary" className="px-2.5" onClick={openEditor}>
          <Target className="h-4 w-4" />
          <span>Goal</span>
        </Button>
      </div>
    );
  }

  if (editing) {
    return (
      <form
        className={variant === "hero" ? "mt-3 rounded-lg border border-border bg-surface-1 p-2" : "mb-2 rounded-lg border border-border bg-surface-1 p-2"}
        onSubmit={(event) => {
          event.preventDefault();
          const budget = parseTokenBudget(tokenBudget);
          setEditing(false);
          void saveThreadGoal(objective, budget, mode);
        }}
      >
        <div className="flex items-start gap-2">
          <Target className="mt-2.5 h-4 w-4 shrink-0 text-muted" />
          <Textarea
            className="min-h-10 flex-1 resize-none border-transparent bg-transparent px-1 py-1 focus:border-transparent focus:ring-0"
            value={objective}
            onChange={(event) => setObjective(event.target.value)}
            placeholder="Goal objective"
            aria-label="Goal objective"
          />
        </div>
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <GoalModeControl mode={mode} onChange={setMode} />
          <div className="ml-auto flex flex-wrap items-center justify-end gap-2">
            <Input
              className="w-36"
              inputMode="numeric"
              min={1}
              type="number"
              value={tokenBudget}
              onChange={(event) => setTokenBudget(event.target.value)}
              placeholder="Token budget"
              aria-label="Goal token budget"
            />
            <div className="flex items-center gap-1">
              <Button
                type="button"
                variant="ghost"
                size="icon"
                aria-label="Cancel goal edit"
                onClick={() => {
                  setEditing(false);
                  closeThreadGoalEditor();
                }}
              >
                <X className="h-4 w-4" />
              </Button>
              <Button type="submit" size="icon" aria-label="Save goal" disabled={!objective.trim()}>
                <Check className="h-4 w-4" />
              </Button>
            </div>
          </div>
        </div>
      </form>
    );
  }

  const visibleGoal = goal;
  const visibleObjective = visibleGoal?.objective ?? draftGoal?.objective;
  if (!visibleObjective) {
    return null;
  }
  const usageLabel = visibleGoal ? goalUsageLabel(visibleGoal) : draftGoalUsageLabel(draftGoal);
  const visibleMode = visibleGoal ? state.currentGoalMode : draftGoal?.mode ?? "standard";
  const modeBadge = goalModeBadge(visibleMode);

  return (
    <div className={variant === "hero" ? "mt-3 rounded-lg border border-border bg-surface-1 p-2" : "mb-2 rounded-lg border border-border bg-surface-1 p-2"}>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <Target className="h-4 w-4 shrink-0 text-muted" />
        <Badge variant={visibleGoal ? goalStatusBadge(visibleGoal.status) : "neutral"}>
          {visibleGoal ? goalStatusLabel(visibleGoal.status) : "draft"}
        </Badge>
        {modeBadge ? <Badge variant={modeBadge.variant}>{modeBadge.label}</Badge> : null}
        <div className="type-body-sm min-w-[160px] flex-1 truncate text-ink">{visibleObjective}</div>
        {usageLabel ? <div className="type-label-sm shrink-0 text-subtle">{usageLabel}</div> : null}
        <div className="ml-auto flex shrink-0 items-center gap-1">
          {visibleGoal?.status === "active" ? (
            <Button type="button" variant="ghost" size="icon" aria-label="Pause goal" onClick={() => void setThreadGoalStatus("paused")}>
              <Pause className="h-4 w-4" />
            </Button>
          ) : visibleGoal?.status === "paused" ? (
            <Button type="button" variant="ghost" size="icon" aria-label="Resume goal" onClick={() => void setThreadGoalStatus("active")}>
              <Play className="h-4 w-4" />
            </Button>
          ) : null}
          <Button type="button" variant="ghost" size="icon" aria-label="Edit goal" onClick={openEditor}>
            <Pencil className="h-4 w-4" />
          </Button>
          <Button type="button" variant="ghost" size="icon" aria-label="Clear goal" onClick={() => void clearThreadGoal()}>
            <Trash2 className="h-4 w-4" />
          </Button>
        </div>
      </div>
    </div>
  );
}

const goalModeOptions: Array<{
  id: ThreadGoalMode;
  label: string;
  title: string;
  icon: LucideIcon;
}> = [
  {
    id: "standard",
    label: "Standard",
    title: "Standard goal mode",
    icon: Gauge
  },
  {
    id: "reviewed",
    label: "Reviewed",
    title: "Reviewer-gated goal mode",
    icon: ShieldCheck
  },
  {
    id: "intensive",
    label: "Intensive",
    title: "Intensive reviewer-gated goal mode",
    icon: Zap
  }
];

function GoalModeControl({
  mode,
  onChange
}: {
  mode: ThreadGoalMode;
  onChange: (mode: ThreadGoalMode) => void;
}) {
  return (
    <div
      className="grid h-8 w-full max-w-[22rem] grid-cols-3 overflow-hidden rounded-md border border-border bg-surface-2 sm:w-[21rem]"
      role="radiogroup"
      aria-label="Goal mode"
    >
      {goalModeOptions.map((option) => {
        const Icon = option.icon;
        const selected = mode === option.id;
        return (
          <button
            key={option.id}
            type="button"
            role="radio"
            aria-checked={selected}
            aria-label={`Goal mode ${option.label}`}
            title={option.title}
            className={cn(
              "type-label-sm inline-flex h-full min-w-0 items-center justify-center gap-1.5 border-r border-border px-2 text-muted transition-colors last:border-r-0 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-focus",
              selected ? "bg-surface-3 text-ink" : "hover:bg-surface-3 hover:text-ink"
            )}
            onClick={() => onChange(option.id)}
          >
            <Icon className="h-3.5 w-3.5 shrink-0" />
            <span className="truncate">{option.label}</span>
          </button>
        );
      })}
    </div>
  );
}

function parseTokenBudget(value: string): number | null {
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }
  const parsed = Number.parseInt(trimmed, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
}

function formatTokenBudget(value: number | null) {
  return value ? String(value) : "";
}

function goalUsageLabel(goal: ThreadGoal) {
  if (!goal.token_budget) {
    return `${goal.tokens_used} tokens`;
  }
  const remaining = Math.max(goal.token_budget - goal.tokens_used, 0);
  return `${remaining}/${goal.token_budget} left`;
}

function draftGoalUsageLabel(goal: DraftThreadGoal | null) {
  return goal?.token_budget ? `0/${goal.token_budget} left` : "";
}

function goalModeBadge(mode: ThreadGoalMode): { label: string; variant: "info" | "primary" } | null {
  switch (mode) {
    case "reviewed":
      return { label: "reviewed", variant: "info" };
    case "intensive":
      return { label: "intensive", variant: "primary" };
    case "standard":
      return null;
  }
}

function goalStatusLabel(status: ThreadGoalStatus) {
  return status.replace("_", " ");
}

function goalStatusBadge(status: ThreadGoalStatus): "neutral" | "success" | "info" | "warning" | "danger" | "primary" {
  switch (status) {
    case "active":
      return "primary";
    case "complete":
      return "success";
    case "blocked":
    case "budget_limited":
    case "usage_limited":
      return "warning";
    case "paused":
      return "neutral";
    default:
      return "info";
  }
}
