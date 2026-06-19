## Intensive mode

**When active:** the strictest operating mode for this goal — everything Reviewed mode requires, plus mandatory delegation.

**Allowed / Forbidden:**
- REQUIRED: delegate exploration and implementation to subagents whenever the work can be separated; keep your own context as the coordinator.
- FORBIDDEN: claiming completion without evidence and QA on real surfaces, or calling `update_goal` complete before an Approved reviewer verdict.

**Completion gate:** before any `update_goal` complete, spawn a reviewer subagent with `agent_type=reviewer` and `fork_turns=none`. To keep the reviewer's context clean, pass only the objective, changed files, diff, and objective evidence.

**Done when:** evidence shows the full objective is done, QA ran on real surfaces, and the reviewer returned Approved. On Rejected — fix every cited issue and re-review; no retry cap, no "good enough". Use `defer_question` for user input needed before honest completion.
