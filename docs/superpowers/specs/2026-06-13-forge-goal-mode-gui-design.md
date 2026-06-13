# Forge Goal Mode GUI Design

## Goal

Expose Forge reviewer-gated completion as a per-goal mode in the desktop GUI.
Users choose the mode when creating or editing a goal, and the runtime applies
the selected mode to that goal only.

The goal model stays thin: `ThreadGoal` does not gain Forge fields, and
`ThreadGoalStatus` does not gain Forge statuses.

## Context

Forge reviewer-gated completion is implemented in the runtime but currently
depends on `EXAGENT_FORGE_REVIEW_GATE_ENABLED`. That makes the feature a
process-level switch, not something a user can control from the desktop app.

The desktop already has a goal editor in `GoalControl`. That is the right
surface for this feature because the decision is goal-specific:

- ordinary goals stay lightweight;
- high-risk goals require reviewer approval before completion;
- especially large goals use the intensive workflow prompt.

## Decision

Add a per-goal `Goal Mode` control with three modes:

- `standard`: current behavior. Completion is not reviewer-gated.
- `reviewed`: completion requires a fresh reviewer approval for the current
  workspace hash. `defer_question` is available for user-owned decisions, and
  unresolved open questions block completion.
- `intensive`: includes `reviewed` behavior and uses the Forge intensive goal
  prompt guidance for subagents, real evidence, QA, clean-context review, and
  deferred questions.

`standard` is the default for new goals. The desktop will not add a global
"Forge on/off" setting in this change.

`EXAGENT_FORGE_REVIEW_GATE_ENABLED` remains an internal kill switch. In desktop
normal operation, Forge services can be available while a `standard` goal still
has no gate. If the kill switch is off, the GUI may still show the saved mode,
but runtime behavior falls back to standard. The mode control is disabled and
non-standard mode badges render in an unavailable state.

## User Experience

### Goal Editor

`GoalControl` adds a compact segmented control below the objective and token
budget inputs:

```text
Mode:  Standard | Reviewed | Intensive
```

The control appears when creating a draft goal and when editing an existing
goal. It uses stable dimensions so the form does not shift when labels change.

Suggested labels:

- Standard
- Reviewed
- Intensive

Suggested tooltips:

- Standard: "Complete goals without reviewer gating."
- Reviewed: "Require fresh reviewer approval before completion."
- Intensive: "Require review and use the stricter Forge workflow prompt."

No explanatory paragraph is added inside the main app surface. The tooltips and
labels carry the behavior.

### Goal Display

The compact goal pill shows mode only when it is not `standard`:

- `Reviewed`
- `Intensive`

This keeps ordinary goals visually quiet. The mode badge sits next to the
status badge and before the objective text.

### Editing Rules

Mode edits are allowed while the session is idle. During an active turn, the
mode control is disabled to avoid changing tool visibility while a model
request is already in progress.

Changing a goal from `reviewed` or `intensive` back to `standard` does not
delete review tickets or open questions. Those records remain available for
history and reports, but `standard` mode no longer treats them as completion
blockers.

Clearing a goal or replacing it with a new objective clears/replaces the active
mode sidecar for that thread.

## Data Model

Keep mode out of `ThreadGoal`.

Use the existing Forge sidecar table concept and extend it from boolean
`intensive` into a mode enum:

```text
standard | reviewed | intensive
```

Durable representation:

- keep table name `forge_goal_modes`;
- add or migrate to a `mode TEXT NOT NULL` column;
- interpret missing rows as `standard`;
- interpret old `intensive = 1` rows as `intensive`;
- interpret old `intensive = 0` rows or missing rows as `standard`.

The Rust runtime exposes a small enum:

```rust
ThreadGoalMode::Standard
ThreadGoalMode::Reviewed
ThreadGoalMode::Intensive
```

The store remains under `src/runtime/forge/goal_modes.rs`.

## Boundary Protocol

Add mode as sidecar metadata in goal boundary responses, not inside
`ThreadGoal`.

Response shape:

```rust
ThreadGoalSetResponse {
    goal: ThreadGoal,
    mode: ThreadGoalMode,
}

ThreadGoalGetResponse {
    goal: Option<ThreadGoal>,
    mode: ThreadGoalMode,
}
```

`ThreadGoalSetParams` accepts an optional `mode`:

- when creating/replacing a goal, omitted mode defaults to `standard`;
- when updating only status, omitted mode preserves the existing mode;
- when updating objective/token budget from the editor, supplied mode replaces
  the sidecar mode.

Thread projections used by the desktop workbench carry goal mode as a separate
field:

```rust
ThreadView {
    goal: Option<ThreadGoal>,
    goal_mode: ThreadGoalMode,
}
```

Runtime events also carry mode outside the goal model:

```rust
ThreadGoalModeUpdated {
    thread_id,
    goal_id,
    mode,
}
```

The desktop store applies this event to its `currentGoalMode`.

## Runtime Behavior

Forge services are available to the desktop runtime independent of whether the
current goal is standard. The active goal mode decides behavior.

Completion gate:

- `standard`: `update_goal(complete)` passes through as today.
- `reviewed`: gate requires no unresolved open questions and a fresh approval.
- `intensive`: same gate as `reviewed`.

Tool visibility:

- `defer_question` is visible only to full-access worker/primary profiles while
  the active goal mode is `reviewed` or `intensive`.
- `submit_review` remains reviewer-only and is available when Forge services
  are available. Direct execution authorization still rejects non-reviewers.

Prompt selection:

- `standard`: existing goal prompt.
- `reviewed`: existing goal prompt plus reviewer-gate behavior.
- `intensive`: Forge intensive prompt.

Rejected-review escalation:

- disabled for `standard`;
- active for `reviewed` and `intensive`.

## Desktop State

Add goal mode alongside the current goal in the workbench store:

```ts
currentGoalMode: ThreadGoalMode
draftGoal.mode?: ThreadGoalMode
```

`saveThreadGoal` accepts mode:

```ts
saveThreadGoal(objective, tokenBudget, mode)
```

`setThreadGoalStatus` does not need a mode argument and preserves the current
mode.

The TypeScript `ThreadGoal` type remains unchanged.

## Error Handling

Invalid modes are rejected at the boundary.

If mode lookup fails during a goal completion attempt, the runtime fails closed
for non-standard modes and reports the storage error. Missing mode rows are not
errors; they mean `standard`.

If the desktop cannot load mode metadata for an existing goal, it shows
`Standard` and preserves the loaded goal. The save path writes the selected mode
explicitly.

If the Forge kill switch is disabled while a goal is set to `reviewed` or
`intensive`, the UI shows the mode badge with a disabled/unavailable state, the
mode editor is disabled, and runtime behavior is standard. This is an
operational fallback, not the normal product path.

## Non-Goals

- No new `ThreadGoal` fields.
- No new `ThreadGoalStatus` variants.
- No global desktop Settings toggle for Forge in this change.
- No reviewer model routing UI.
- No desktop UI for manually opening or closing review tickets.
- No deletion UI for old review tickets or resolved questions.

## Testing

Rust:

- mode store defaults missing rows to `standard`;
- old intensive rows map to `intensive`;
- `ThreadGoalSetParams` create/update persists mode without changing
  `ThreadGoal`;
- status-only goal updates preserve mode;
- clearing/replacing goals clears or replaces sidecar mode;
- completion gate is inactive for `standard`;
- completion gate is active for `reviewed` and `intensive`;
- `defer_question` is hidden for `standard` and visible for reviewed/intensive
  worker profiles;
- intensive prompt is selected only for `intensive`;
- architecture guard still pins `ThreadGoal` and `ThreadGoalStatus`.

Desktop:

- goal editor renders the three mode options;
- new goal defaults to `standard`;
- saving a goal sends the selected mode;
- editing an existing goal loads and preserves mode;
- mode badge appears for `reviewed` and `intensive`, not for `standard`;
- mode control is disabled or view-only during running turns;
- runtime mode events update the visible badge.

Build/verification:

- `cargo test`
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml`
- `cd apps/desktop && npm test`
- `cd apps/desktop && npm run build`
