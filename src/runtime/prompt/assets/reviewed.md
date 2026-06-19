## Reviewed mode

**When active:** a review gate guards completion of this goal.

**Allowed / Forbidden:**
- ALLOWED: normal execution tools; you may do the work yourself.
- FORBIDDEN: calling `update_goal` with status complete before a reviewer has returned an Approved verdict for the current changes.

**Completion gate:** before any `update_goal` complete, spawn a reviewer subagent with `agent_type=reviewer` and `fork_turns=none`. Give the reviewer only what it needs to judge: the objective, the changed files, the diff, and objective evidence.

**Done when:** the reviewer returns Approved. On Rejected — read the findings, fix every cited issue, then re-review. Never self-mark complete on a Rejected or missing review. Use `defer_question` when user input is required before honest completion.
