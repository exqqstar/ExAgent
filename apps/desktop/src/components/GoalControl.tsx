import { useEffect, useState } from "react";
import { Check, Pause, Pencil, Play, Target, Trash2, X } from "lucide-react";
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
import type { ThreadGoal, ThreadGoalStatus } from "@/types";

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
  const [editing, setEditing] = useState(false);
  const [objective, setObjective] = useState("");
  const [tokenBudget, setTokenBudget] = useState("");

  useEffect(() => {
    if (state.goalEditorOpen) {
      setObjective(goal?.objective ?? state.composerValue.trim());
      setTokenBudget(goal?.token_budget ? String(goal.token_budget) : "");
      setEditing(true);
    }
  }, [goal?.objective, goal?.token_budget, state.composerValue, state.goalEditorOpen]);

  useEffect(() => {
    if (state.goalEditorOpen) {
      return;
    }
    if (!goal) {
      return;
    }
    setObjective(goal.objective);
    setTokenBudget(goal.token_budget ? String(goal.token_budget) : "");
    setEditing(false);
  }, [goal?.goal_id, goal?.objective, goal?.token_budget, state.goalEditorOpen]);

  if (!state.activeSessionId) {
    return null;
  }

  const openEditor = () => {
    setObjective(goal?.objective ?? state.composerValue.trim());
    setTokenBudget(goal?.token_budget ? String(goal.token_budget) : "");
    setEditing(true);
  };

  if (!goal && !editing) {
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
          void saveThreadGoal(objective, budget);
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
        <div className="mt-2 flex flex-wrap items-center justify-between gap-2">
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
      </form>
    );
  }

  const visibleGoal = goal;
  if (!visibleGoal) {
    return null;
  }

  return (
    <div className={variant === "hero" ? "mt-3 rounded-lg border border-border bg-surface-1 p-2" : "mb-2 rounded-lg border border-border bg-surface-1 p-2"}>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <Target className="h-4 w-4 shrink-0 text-muted" />
        <Badge variant={goalStatusBadge(visibleGoal.status)}>{goalStatusLabel(visibleGoal.status)}</Badge>
        <div className="type-body-sm min-w-[160px] flex-1 truncate text-ink">{visibleGoal.objective}</div>
        <div className="type-label-sm shrink-0 text-subtle">{goalUsageLabel(visibleGoal)}</div>
        <div className="ml-auto flex shrink-0 items-center gap-1">
          {visibleGoal.status === "active" ? (
            <Button type="button" variant="ghost" size="icon" aria-label="Pause goal" onClick={() => void setThreadGoalStatus("paused")}>
              <Pause className="h-4 w-4" />
            </Button>
          ) : visibleGoal.status === "paused" ? (
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

function parseTokenBudget(value: string): number | null {
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }
  const parsed = Number.parseInt(trimmed, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
}

function goalUsageLabel(goal: ThreadGoal) {
  if (!goal.token_budget) {
    return `${goal.tokens_used} tokens`;
  }
  const remaining = Math.max(goal.token_budget - goal.tokens_used, 0);
  return `${remaining}/${goal.token_budget} left`;
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
