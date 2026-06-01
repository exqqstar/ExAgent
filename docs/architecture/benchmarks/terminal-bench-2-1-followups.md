# Terminal-Bench 2.1 Follow-Ups

## Context

This note records issues found while running a small Terminal-Bench 2.1 subset
with the local ExAgent harness and DeepSeek.

Run shape:

- Benchmark: `terminal-bench/terminal-bench-2-1`
- Agent: local ExAgent installed-agent adapter for Harbor
- Model: `deepseek-v4-flash`
- Thinking mode: `high`
- Policy mode: `off`
- Max turns: `EXAGENT_MAX_TURNS=24`

Small subset:

- `terminal-bench/fix-git`
- `terminal-bench/prove-plus-comm`
- `terminal-bench/cobol-modernization`
- `terminal-bench/raman-fitting`
- `terminal-bench/openssl-selfsigned-cert`

Observed results:

- Raw 5-task run: `3/5`, mean reward `0.600`
- Adjusted result after fixing the adapter cwd issue and rerunning
  `prove-plus-comm`: `4/5`, mean reward `0.800`
- Main remaining failed task: `terminal-bench/raman-fitting`

Local artifacts:

- Raw job: `.exagent-bench/jobs/tb21-exagent-ds-5task-mt24/`
- Proof rerun: `.exagent-bench/jobs/tb21-exagent-ds-prove-mt24-cwdfix/`
- Adjusted metrics: `.exagent-bench/metrics/tb21-exagent-ds-5task-adjusted.metrics.json`
- Adjusted summary: `.exagent-bench/metrics/tb21-exagent-ds-5task-adjusted.metrics.md`

Diagnostic 10-task subset:

- `terminal-bench/break-filter-js-from-html`
- `terminal-bench/constraints-scheduling`
- `terminal-bench/crack-7z-hash`
- `terminal-bench/custom-memory-heap-crash`
- `terminal-bench/git-leak-recovery`
- `terminal-bench/kv-store-grpc`
- `terminal-bench/multi-source-data-merger`
- `terminal-bench/nginx-request-logging`
- `terminal-bench/regex-log`
- `terminal-bench/sqlite-with-gcov`

Observed results:

- Diagnostic 10-task run: `7/10`, mean reward `0.700`
- Failed tasks:
  `kv-store-grpc`,
  `crack-7z-hash`,
  `custom-memory-heap-crash`
- Local job: `.exagent-bench/jobs/tb21-exagent-ds-diagnostic10-mt24/`
- Metrics: `.exagent-bench/metrics/tb21-exagent-ds-diagnostic10-mt24.metrics.json`
- Summary: `.exagent-bench/metrics/tb21-exagent-ds-diagnostic10-mt24.metrics.md`

The `.exagent-bench/` directory is local benchmark output and should not be
treated as source architecture state.

## Issue 1: Finalization And Max-Turn Exhaustion

### What Happened

The runtime loop stops only when the model returns an assistant turn with no
tool calls. If every assistant turn includes at least one tool call, the loop
continues until `agent.max_turns()`.

In the Cobol task, the verifier passed, but ExAgent still exited with:

```text
Agent reached max turns (24) without a final assistant turn
```

This means the task state was good enough for the benchmark verifier, but the
agent never produced a final no-tool assistant turn.

This did not happen for every task. `fix-git`, `openssl-selfsigned-cert`, and
the fixed `prove-plus-comm` run ended normally.

### Why It Matters

- Wastes tokens and wall-clock time after useful work is already complete.
- Produces a bad CLI exit code even when the external verifier passes.
- Can become a false failure in harnesses that treat non-zero agent exit as
  final, rather than letting the verifier judge the filesystem state.
- Makes long tasks more likely to hit max-turn limits for procedural reasons,
  not capability reasons.

### Candidate Fixes

- Add a finalization pressure path near the end of the turn budget:
  ask the model to stop exploration and provide a final answer unless one
  clearly necessary command remains.
- Detect repeated verification loops:
  if recent commands are only `ls`, `cat`, `diff`, test reruns, or equivalent
  checks with no material file changes, bias toward finalization.
- Track `max_turns_reached` as a first-class runtime event or rollout metric.
- Preserve enough state on max-turn failure to distinguish:
  "task likely complete but no final answer" from "task still incomplete."
- Consider a benchmark adapter mode where verifier outcome is still recorded
  even when ExAgent exits non-zero. The temporary adapter already moved in this
  direction.

### Metrics To Keep

- `assistant_turns`
- `declared_tool_calls`
- `max_turns_reached`
- `final_assistant_missing`
- repeated command count
- commands after the last file mutation
- verifier reward when ExAgent exit code is non-zero

## Issue 2: Workspace Preflight And Initial Cwd

### What Happened

Two different workspace problems appeared:

1. `prove-plus-comm` uses `/workspace`, not `/app`.
2. `fix-git` had `/app`, but the actual git repository was under
   `/app/personal-site`.

The first was a hard adapter bug. The command wrapper tried to start with
`cwd=/app`, so Harbor failed before ExAgent could run.

The second was an efficiency problem. ExAgent started with git commands in
`/app`, saw "not a git repository", then recovered by listing files and finding
the real repository.

### Why It Matters

- Wrong outer cwd can make a benchmark trial invalid before the agent starts.
- Wrong project root wastes early tool calls and tokens.
- In harder tasks, early wrong assumptions can send the model down the wrong
  path before it has a good environment model.

### Candidate Fixes

- Formalize benchmark adapter cwd selection:
  prefer `/app` if it exists, else `/workspace`, else `/`.
- Pass the resolved cwd into ExAgent as the actual runtime cwd and workspace
  root, instead of relying on the model to discover it after startup.
- Add a workspace preflight summary before the first model call:
  current cwd, top-level entries, `.git` location, likely project roots, and
  common task files.
- Keep preflight concise and structured. It should orient the model, not dump a
  directory tree.
- Consider a reusable `workspace_scan` tool or runtime-injected context item.

### Metrics To Keep

- selected benchmark cwd
- first successful project root
- number of commands before project root discovery
- number of "not a git repository" or missing-file errors in the first N tool
  calls
- whether `.git` was in cwd or a child directory

## Issue 3: Tool Output Size, Repeated Commands, And In-Turn Compaction

### What Happened

`raman-fitting` consumed much more context than the other tasks:

- Total tokens: about `543,792`
- Tool calls: `24`
- Repeated Python fitting attempts with large inline scripts
- Repeated data parsing and analysis outputs

Current `run_command` behavior truncates stdout and stderr to
`max_output_bytes`, then puts the truncated strings directly into the
model-visible tool result. That is useful for transparency, but expensive for
benchmark loops that generate noisy output repeatedly.

### Why It Matters

- Long stdout/stderr becomes model context immediately.
- Repeated command output compounds across the same user turn.
- Context growth can make the model less focused even before the formal
  context window is full.
- Current compaction is not enough for long single-turn tool loops, because the
  prompt grows inside the same user turn.

### Candidate Fixes

Add a tool-output projection layer before output enters model context:

- Store full stdout/stderr in rollout or tool artifacts.
- Return a compact model-visible projection:
  `exit_code`, `cwd`, byte counts, first lines, last lines, and key error
  snippets.
- Detect output classes:
  package install logs, pytest output, tracebacks, tabular data, directory
  listings, binary/hex dumps, and long generated source files.
- Summarize known noisy classes differently:
  package logs should collapse to installed/failed packages;
  tracebacks should keep exception type, file, line, and final error;
  data previews should keep shape, ranges, and samples.
- Include truncation metadata:
  `stdout_bytes`, `stderr_bytes`, `stdout_truncated`, `stderr_truncated`.

Add repeated command detection:

- Normalize command text into a fingerprint.
- Track repeated commands within a turn.
- If the same command returns the same output, return "same as previous" plus
  the previous call id.
- If it returns different output, show only the diff or changed summary.

Add in-turn compaction:

- Compact or summarize tool history inside a single long user turn, not only
  between user turns.
- Trigger by prompt token growth, repeated command patterns, or tool-output
  byte count.

### Metrics To Keep

- raw stdout/stderr bytes
- model-visible stdout/stderr bytes
- truncation flags
- repeated command fingerprints
- repeated output fingerprints
- cumulative tool-output bytes per turn
- compaction count within a turn
- token usage before and after compaction

## Issue 4: Data-Analysis Workflow Support

### What Happened

`raman-fitting` failed with a wrong numerical fit. The final `results.json`
existed, but the fitted values were far outside expected Raman peak ranges.

This was not simply a missing shell tool problem. The environment had enough
basic capability to run Python and install/use scientific packages. The failure
was more about workflow:

- identifying the data format,
- understanding the x/y columns,
- locating the G and 2D peak windows,
- fitting local regions rather than the wrong global scale,
- validating output values against expected physical ranges.

### Why It Matters

Many Terminal-Bench tasks require domain-shaped workflows. A generic
`run_command` tool is enough to execute code, but not always enough to keep the
agent on the right analysis path.

### Candidate Fixes

- Add a lightweight data profiling path:
  row count, column count, numeric ranges, percentiles, missing values, and
  sample rows.
- Add a plot or visual summary path:
  save plot artifacts and return textual peak/range summaries to the model.
- Add a scientific-computing guidance context:
  for curve fitting, first inspect ranges, isolate local windows, choose
  initial guesses, fit, then sanity-check parameters.
- Consider a structured helper for common numerical tasks, but avoid hardcoding
  benchmark-specific answers.
- Teach the harness to keep generated plots/artifacts available for inspection
  without stuffing them into text context.

### Metrics To Keep

- whether data profiling happened before fitting
- whether the agent inspected numeric ranges
- whether local fitting windows were used
- number of fitting attempts
- final result sanity checks
- generated analysis artifacts

## Issue 5: Workspace-Scoped Absolute Paths

### What Happened

Several Terminal-Bench prompts refer to files as absolute paths under the
workspace root, for example `/app/main.cpp` or `/app/kv-store.proto`.
The model naturally passed those paths to file tools, but `read_file` and
`write_file` rejected them with:

```text
Absolute paths are not allowed
```

This happened in both failed and passing tasks. The agent often recovered by
switching to relative paths, but it spent extra tool calls and context on a
problem that the runtime already had enough information to resolve safely.

### Why It Matters

- Benchmark and container tasks commonly describe files with absolute paths.
- Rejecting workspace-internal absolute paths creates avoidable early errors.
- The model learns a misleading distinction:
  absolute paths work in `run_command`, but not in file tools.
- This costs tool calls and can matter on tasks already close to the turn limit.

### Candidate Fixes

- Allow absolute paths that canonicalize under `workspace_root`.
- Normalize accepted absolute paths to workspace-relative paths internally.
- Keep rejecting paths outside `workspace_root`.
- Record both `requested_path` and `normalized_path` in tool metadata.
- Add tests for `/app/file`, `/workspace/file`, symlink escapes, and `..`
  traversal.

### Metrics To Keep

- absolute-path file tool attempts
- accepted workspace-internal absolute paths
- rejected out-of-workspace absolute paths
- first N file-tool path errors per task

## Issue 6: Timeout Cleanup Must Kill Process Groups

### What Happened

`crack-7z-hash` repeatedly launched long-running password cracking or password
trial commands. The model-visible tool result reported timeouts, but during the
run child processes continued to live after the shell command timed out.

The task ultimately failed because no `/app/solution.txt` was written. It also
spent `472s` of agent time, with `4` command timeouts and `12` tool errors.

### Why It Matters

- Killing only the direct child shell is not enough for commands that spawn
  grandchildren such as `john`, `7z`, package managers, servers, or test
  runners.
- Orphaned work consumes CPU and wall-clock time after the agent thinks the
  command has ended.
- Later commands may observe stale background processes or locked files.
- Interrupt and timeout semantics become unreliable for benchmark harnesses and
  app-server users.

### Candidate Fixes

- Spawn each non-persistent command in its own process group or session.
- On timeout, send a graceful termination signal to the whole process group.
- Escalate to force-kill after a short grace period.
- Record timeout cleanup metadata:
  root pid, process group id, killed pid count, grace timeout, and exit status.
- Apply the same model to explicit interrupts.
- Keep persistent exec sessions separate from ordinary timed commands so
  intended background processes are managed by a named lifecycle.

### Metrics To Keep

- timed-out commands
- timeout cleanup success or failure
- killed descendant count
- remaining descendants after cleanup
- commands started as persistent sessions
- commands that background with `&` outside a managed session

## Issue 7: Strict Contract Fidelity For Protocol Tasks

### What Happened

`kv-store-grpc` built and started a real gRPC server, but failed the verifier
because the generated proto used `SetValRequest.val` where the task explicitly
required `SetValRequest.value`.

The verifier failure was:

```text
Protocol message SetValRequest has no "value" field.
```

This is not primarily a runtime crash. It is a spec fidelity failure:
the implemented service was close, but not byte-for-byte compatible with the
requested protocol contract.

### Why It Matters

- Many benchmark tasks are strict about names, field shapes, ports, filenames,
  and command-line behavior.
- "Close enough" implementations can pass superficial self-tests but fail
  hidden or verifier checks.
- The harness currently gives the model generic tools, but little structured
  pressure to extract and preserve exact contracts from the prompt.

### Candidate Fixes

- Add a lightweight contract extraction step for tasks with protocols,
  schemas, CLIs, APIs, filenames, or ports.
- Keep extracted contracts visible as compact context during implementation.
- Encourage self-tests that use the exact names from the prompt, not names
  inferred from nearby response fields.
- Consider a "spec checklist" runtime hint for benchmark mode:
  files, services, RPC names, message names, field names, ports, output paths.
- Record contract deltas when verifier output points to name mismatches.

### Metrics To Keep

- contract items extracted
- contract items self-tested
- verifier failures involving missing fields, missing files, wrong names, or
  wrong ports
- number of task-prompt identifiers copied into generated files

## Issue 8: Long Debug Loops Need Budget Awareness

### What Happened

`custom-memory-heap-crash` reached `EXAGENT_MAX_TURNS=24` without a final
assistant turn. The agent did useful investigation, identified a plausible
libstdc++ facet-registration failure mode, and wrote a final `user.cpp`, but
hit the turn limit immediately after that write. It did not compile and verify
the final attempt before the runtime stopped.

The external verifier still failed:

```text
Release build crashed! This indicates the bug is not fixed.
```

This is related to Issue 1, but it is not only "forgot to final answer."
Here the agent was still in a real debugging loop and spent many turns on
low-yield inspection commands.

### Why It Matters

- Hard debugging tasks need exploration, but not unlimited exploration.
- Late-stage file writes without immediate verification create ambiguous
  failures.
- The runtime has no explicit "you have N tool-call rounds left" pressure.
- The model can continue with disassembly or source archaeology even when it
  should converge on a testable patch.

### Candidate Fixes

- Expose remaining turn budget in runtime context after a threshold.
- Add a late-budget mode:
  prioritize one concrete patch, compile, run verifier-shaped tests, then stop.
- Track the last file mutation and require a verification attempt before
  another low-signal inspection command when budget is low.
- Mark max-turn failures with whether the last operation was:
  file mutation, test run, read-only inspection, or package install.
- Consider per-task adaptive max turns, but only after improving budget
  behavior.

### Metrics To Keep

- remaining turns when the last file mutation happens
- commands after last file mutation
- whether final mutation was verified
- low-yield inspection count after first plausible diagnosis
- max-turn failure state category

## Priority Order

1. Timeout cleanup and interrupt semantics for process groups.
2. Finalization and max-turn budget awareness.
3. Workspace preflight, runtime cwd selection, and workspace-scoped absolute
   file paths.
4. Tool-output projection, repeated command detection, and in-turn compaction.
5. Strict contract extraction for protocol and schema-heavy tasks.
6. Data-analysis workflow helpers.

The first three are harness/runtime correctness and reliability issues.
Tool-output projection is token economics and long-run stability. Contract and
data-analysis helpers improve capability on recurring benchmark task classes
without hardcoding a single benchmark answer.
