# ADR-0006: Model Turn Completion As Runtime Lifecycle

## Status

Accepted

## Context

The model loop naturally completes when a sampling response produces no tool
calls. Before this ADR, ExAgent recorded `TurnCompleted` after
`run_session_turn` returned a final assistant turn, but if every sampled
assistant turn included at least one tool call, the implementation stopped only
when `AgentConfig.max_turns` was reached and returned a runtime error.

That hard limit is not a good completion strategy. Benchmark runs showed cases
where useful work was done, but the agent continued to inspect or verify until
it hit `EXAGENT_MAX_TURNS`. This creates a bad distinction: the filesystem may
be correct while the runtime reports a failure because no final no-tool
assistant message arrived. It also makes long tasks fail for a procedural reason
instead of because the task is actually impossible.

Codex separates two concepts and does not use a fixed tool-round limit as the
normal lifecycle controller:

- model completion: the model no longer asks for follow-up work
- runtime turn completion: the task runner has finished and emits a lifecycle
  event with the last assistant message, timing, and metrics

The runtime completion event is not an extra forced model sample. It is a
uniform lifecycle boundary around task execution. Runaway-loop protection, if
added later, should be an explicit watchdog or budget policy rather than the
default turn-completion mechanism.

## Decision

ExAgent should treat turn completion as a runtime lifecycle event, while using a
Codex-like follow-up loop by default:

- The model loop continues while the model asks for follow-up work through tool
  calls.
- The model loop is complete when a model response has no tool calls.
- `TurnCompleted` remains a runtime event emitted after task cleanup, rollout
  flush, token metrics, and live-state updates.
- `AgentConfig.max_turns` and `EXAGENT_MAX_TURNS` are removed from the default
  lifecycle path.
- ExAgent should not inject "remaining turns" budget context in the normal path;
  that context depends on a fixed tool-round budget that should not exist by
  default.
- ExAgent should not add an unconditional extra "finalization sample"; that can
  mask real loops and spend more tokens after a guardrail has already fired.
- If a future watchdog is needed, it should be named and reported as an
  exceptional runtime guard, such as `LoopGuardTriggered` or
  `RuntimeBudgetExceeded`, and should be configurable independently from normal
  turn completion.

## Consequences

- Clients get a reliable lifecycle event even as turn execution grows more
  complex.
- Long tasks are not failed solely because they require many tool-call rounds.
- The model still owns semantic final answers.
- Runaway-loop protection is deferred to a future explicit watchdog, interruption
  policy, or cost/runtime budget.
- Tests that need non-completing model behavior should use explicit timeouts or
  interruption instead of relying on a default max-turn error.

## Affected Modules

- `src/runtime/thread_session/turn.rs`
- `src/config.rs`
- `src/runtime/agent.rs`
- `tests/config_defaults.rs`
- `tests/thread_runtime.rs`

## Related Docs

- `docs/architecture/benchmarks/terminal-bench-2-1-followups.md`
- `docs/plans/2026-06-01-exagent-runtime-hardening-followups.md`
