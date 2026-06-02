# ExAgent Runtime Hardening Follow-Ups

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden ExAgent's shell execution, turn completion, workspace paths, and command output behavior using the benchmark findings as the driver.

**Architecture:** Keep the current runtime shape. Add focused boundaries where the existing implementation is too implicit: process cleanup for shell commands, lifecycle completion for turns, normalized workspace path resolution, and model-visible tool-output projection.

**Tech Stack:** Rust, Tokio process management, serde runtime events, rollout persistence, existing app-server boundary tests.

## Implementation Status

Implemented on 2026-06-01:

- Task 1: shared Unix process-group cleanup for one-shot timeouts and
  persistent exec termination.
- Task 2: Codex-like follow-up loop; removed `max_turns` from the default
  runtime lifecycle path.
- Task 3: workspace-scoped absolute path resolver with canonical containment
  checks and file tool path metadata.
- Task 4: one-shot command output projection with byte counts, truncation
  flags, command metadata, and duration metadata.

Verification: `cargo test` passed after implementation.

---

## Decision References

- `docs/architecture/adr/0005-use-process-groups-for-command-cleanup.md`
- `docs/architecture/adr/0006-model-turn-completion-as-runtime-lifecycle.md`
- `docs/architecture/adr/0007-allow-workspace-scoped-absolute-paths.md`
- `docs/architecture/adr/0008-project-tool-output-before-model-context.md`
- `docs/architecture/benchmarks/terminal-bench-2-1-followups.md`

## File Structure

- Create `src/runtime/process_cleanup.rs`: shared Unix process-group configuration and process-tree cleanup helpers, with best-effort non-Unix fallback.
- Modify `src/tools/run_command.rs`: use shared cleanup helpers for one-shot command spawn, timeout, cleanup, capture, and one-shot output projection.
- Create `src/tools/output_projection.rs`: byte-based head/tail projection helper for one-shot command output.
- Modify `src/runtime/exec_session.rs`: use shared cleanup helpers when starting persistent sessions and when explicitly terminating them.
- Modify `src/workspace.rs`: replace the absolute-path ban with structured workspace-contained normalization.
- Modify `src/tools/read_file.rs` and `src/tools/write_file.rs`: consume the structured resolver and include requested/normalized path metadata.
- Modify `src/runtime/thread_session/turn.rs`: replace the bounded max-turn loop with a Codex-like follow-up loop.
- Modify `src/config.rs` and `src/runtime/agent.rs`: remove or deprecate `max_turns` as a default lifecycle control.
- Test `tests/run_command.rs`: process-group cleanup and output projection.
- Test `tests/file_tools.rs`: workspace-scoped absolute paths and symlink escapes.
- Test `src/runtime/thread_session/turn.rs` unit tests and `tests/config_defaults.rs`: follow-up loop completion and config default semantics.

## Task 1: Shared Process Cleanup Primitive

**Files:**
- Create: `src/runtime/process_cleanup.rs`
- Modify: `src/runtime/mod.rs`
- Modify: `src/tools/run_command.rs`
- Modify: `src/runtime/exec_session.rs`
- Test: `tests/run_command.rs`
- Test: `tests/exec_session.rs`

- [ ] **Step 1: Add a failing one-shot timeout cleanup test**

Add a Unix-only test that starts a command with a background child, times out,
then checks the background child is gone:

```rust
#[cfg(unix)]
#[tokio::test]
async fn run_command_timeout_kills_background_children() {
    let (dir, ctx) = test_context();
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let pid_file = dir.path().join("child.pid");
    let command = format!("sleep 60 & echo $! > {}; sleep 60", pid_file.display());

    let result = registry
        .execute(
            ToolCall {
                id: "call_timeout_group".into(),
                name: "run_command".into(),
                arguments: json!({
                    "command": command,
                    "timeout_secs": 1
                }),
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status.as_str(), "error");
    assert_eq!(result.meta.as_ref().unwrap()["timed_out"], true);

    let child_pid = std::fs::read_to_string(pid_file).unwrap();
    let status = std::process::Command::new("kill")
        .arg("-0")
        .arg(child_pid.trim())
        .status()
        .unwrap();
    assert!(!status.success(), "background child should be gone after timeout");
}
```

- [ ] **Step 2: Run the failing test**

Run:

```bash
cargo test --test run_command run_command_timeout_kills_background_children
```

Expected: FAIL because timeout returns before killing the process group.

- [ ] **Step 3: Add a failing persistent terminate cleanup test**

Add a Unix-only test in `tests/exec_session.rs` that starts a persistent command
with a background child, terminates it, and verifies the background child is
gone:

```rust
#[cfg(unix)]
#[tokio::test]
async fn persistent_exec_session_terminate_kills_background_children() {
    let (dir, _thread_id, ctx) = test_context();
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let pid_file = dir.path().join("persistent-child.pid");
    let command = format!("sleep 60 & echo $! > {}; sleep 60", pid_file.display());

    let started = registry
        .execute(
            ToolCall {
                id: "call_start_group".into(),
                name: "run_command".into(),
                arguments: json!({
                    "command": command,
                    "persistent": true
                }),
            },
            Some(&ctx),
        )
        .await;

    let exec_session_id = started.meta.as_ref().unwrap()["exec_session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let deadline = Instant::now() + Duration::from_secs(2);
    while !pid_file.exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(pid_file.exists(), "persistent child pid should be written");

    let terminated = registry
        .execute(
            ToolCall {
                id: "call_terminate_group".into(),
                name: "run_command".into(),
                arguments: json!({
                    "exec_session_id": exec_session_id,
                    "terminate": true
                }),
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(terminated.status.as_str(), "success");
    assert_eq!(terminated.meta.as_ref().unwrap()["lifecycle"], "terminated");

    let child_pid = std::fs::read_to_string(pid_file).unwrap();
    let status = std::process::Command::new("kill")
        .arg("-0")
        .arg(child_pid.trim())
        .status()
        .unwrap();
    assert!(
        !status.success(),
        "persistent background child should be gone after terminate"
    );
}
```

- [ ] **Step 4: Run the failing persistent test**

Run:

```bash
cargo test --test exec_session persistent_exec_session_terminate_kills_background_children
```

Expected: FAIL because persistent `terminate` kills only the direct child.

- [ ] **Step 5: Add the shared cleanup module**

Create `src/runtime/process_cleanup.rs`:

```rust
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessCleanupReason {
    Timeout,
    Terminate,
    Interrupt,
    RuntimeShutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessCleanupReport {
    pub root_pid: Option<u32>,
    pub process_group_id: Option<i32>,
    pub graceful_signal_sent: bool,
    pub force_kill_sent: bool,
    pub direct_child_kill_sent: bool,
    pub success: bool,
}

#[cfg(unix)]
pub(crate) fn configure_process_group(command: &mut tokio::process::Command) {
    use std::os::unix::process::CommandExt;

    unsafe {
        command.pre_exec(|| {
            let rc = libc::setpgid(0, 0);
            if rc == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
pub(crate) fn configure_process_group(_command: &mut tokio::process::Command) {}

#[cfg(unix)]
pub(crate) async fn cleanup_child_process_tree(
    child: &mut tokio::process::Child,
    _reason: ProcessCleanupReason,
    grace: Duration,
) -> ProcessCleanupReport {
    let root_pid = child.id();
    let process_group_id = root_pid.and_then(|pid| {
        let pgid = unsafe { libc::getpgid(pid as libc::pid_t) };
        (pgid > 0).then_some(pgid)
    });

    let mut graceful_signal_sent = false;
    let mut force_kill_sent = false;

    if let Some(pgid) = process_group_id {
        graceful_signal_sent = unsafe { libc::killpg(pgid, libc::SIGTERM) } == 0;
        let _ = tokio::time::timeout(grace, child.wait()).await;
        if child.try_wait().ok().flatten().is_none() {
            force_kill_sent = unsafe { libc::killpg(pgid, libc::SIGKILL) } == 0;
        }
    }

    let direct_child_kill_sent = child.start_kill().is_ok();
    let success = child.wait().await.is_ok();

    ProcessCleanupReport {
        root_pid,
        process_group_id,
        graceful_signal_sent,
        force_kill_sent,
        direct_child_kill_sent,
        success,
    }
}

#[cfg(not(unix))]
pub(crate) async fn cleanup_child_process_tree(
    child: &mut tokio::process::Child,
    _reason: ProcessCleanupReason,
    _grace: Duration,
) -> ProcessCleanupReport {
    let root_pid = child.id();
    let direct_child_kill_sent = child.start_kill().is_ok();
    let success = child.wait().await.is_ok();

    ProcessCleanupReport {
        root_pid,
        process_group_id: None,
        graceful_signal_sent: false,
        force_kill_sent: false,
        direct_child_kill_sent,
        success,
    }
}
```

Add the module to `src/runtime/mod.rs`:

```rust
pub(crate) mod process_cleanup;
```

If `libc` is not already available as a direct dependency, add it to `Cargo.toml`.

- [ ] **Step 6: Use the helper for one-shot timeout**

Update `run_one_shot_command` to call `configure_process_group(&mut command)`
before spawn and use `child.wait()` plus explicit stdout/stderr capture, rather
than `wait_with_output`, so the timeout branch can call:

```rust
let cleanup = cleanup_child_process_tree(
    &mut child,
    ProcessCleanupReason::Timeout,
    Duration::from_millis(750),
)
.await;
```

Include the cleanup report in timeout metadata.

- [ ] **Step 7: Use the helper for persistent start and terminate**

In `ExecSessionManager::start`, call `configure_process_group(&mut command)`
before spawning the persistent child. In `terminate`, replace direct
`child.kill().await` with:

```rust
let _cleanup = cleanup_child_process_tree(
    &mut child,
    ProcessCleanupReason::Terminate,
    Duration::from_millis(750),
)
.await;
```

Do not change persistent exec's normal lifecycle: it still stays alive across
turns, accepts stdin, returns poll snapshots, and remains live-only across cold
replay.

- [ ] **Step 8: Verify**

Run:

```bash
cargo test --test run_command run_command_timeout_kills_background_children
cargo test --test run_command
cargo test --test exec_session persistent_exec_session_terminate_kills_background_children
cargo test --test exec_session
cargo test
```

Expected: PASS.

## Task 2: Codex-Like Follow-Up Loop

**Files:**
- Modify: `src/runtime/thread_session/turn.rs`
- Modify: `src/config.rs`
- Modify: `src/runtime/agent.rs`
- Test: `tests/config_defaults.rs`
- Test: `src/runtime/thread_session/turn.rs`

- [ ] **Step 1: Add a failing follow-up-loop test**

Add a runtime test where the mock LLM returns more tool-call turns than the old
default `max_turns = 12`, then finally returns an assistant turn with no tool
calls. Assert the runtime completes normally with `TurnCompleted` and the final
assistant turn.

- [ ] **Step 2: Replace the bounded loop**

Change `run_session_turn` from:

```rust
for _ in 0..agent.max_turns() {
    // sample, record, execute tools
}
```

to a Codex-like follow-up loop:

```rust
loop {
    // sample, record, execute tools
    if turn.tool_calls.is_empty() {
        return Ok(turn);
    }
}
```

The normal completion condition remains a no-tool assistant turn. Do not add an
extra finalization sample.

- [ ] **Step 3: Remove max-turn as default lifecycle config**

Remove `Agent::max_turns()` and update `AgentConfig` / `tests/config_defaults.rs`
so `EXAGENT_MAX_TURNS` is no longer a default lifecycle control. If keeping the
field temporarily is lower risk, mark it deprecated and unused by the turn loop.

- [ ] **Step 4: Do not add late-budget context**

Do not inject `remaining_turns <= 3` context in this task. A future runtime
watchdog or budget policy can add explicit guard context if it is introduced.

- [ ] **Step 5: Verify**

Run:

```bash
cargo test --test thread_runtime
cargo test --test config_defaults
cargo test
```

Expected: PASS.

## Task 3: Workspace-Scoped Absolute File Paths

**Files:**
- Modify: `src/workspace.rs`
- Modify: `src/tools/read_file.rs`
- Modify: `src/tools/write_file.rs`
- Test: `tests/file_tools.rs`

- [ ] **Step 1: Add failing absolute-path tests**

Add tests for:

- absolute path inside workspace is accepted
- absolute path outside workspace is rejected
- symlink from workspace to outside workspace is rejected
- metadata includes requested and normalized path

- [ ] **Step 2: Replace the absolute-path ban with a structured resolver**

Update `resolve_workspace_path` to return a structured result instead of a bare
`PathBuf`, including:

```rust
pub struct ResolvedWorkspacePath {
    pub requested_path: String,
    pub normalized_path: PathBuf,
    pub canonical_path: PathBuf,
    pub was_absolute: bool,
}
```

Accept absolute paths only when they resolve under the canonical workspace root.
For existing files, canonicalize the target. For writes to missing files,
canonicalize the nearest existing parent and then append the missing tail.
Reject symlink escapes in this early implementation.

- [ ] **Step 3: Update tool metadata**

For read/write success, return:

```json
{
  "requested_path": "...",
  "normalized_path": "...",
  "was_absolute": true,
  "path": "..."
}
```

Keep `path` temporarily for compatibility.

- [ ] **Step 4: Verify**

Run:

```bash
cargo test --test file_tools
cargo test
```

Expected: PASS.

## Task 4: Command Output Projection

**Files:**
- Create: `src/tools/output_projection.rs`
- Modify: `src/tools/run_command.rs`
- Test: `tests/run_command.rs`

- [ ] **Step 1: Add projection tests**

Add tests proving long one-shot stdout/stderr returns compact model-visible
content while meta keeps byte counts and truncation flags. Include a case where
the failure detail appears near the tail and must remain visible.

- [ ] **Step 2: Add an output projection helper**

Create a helper that accepts stdout, stderr, max bytes, exit code, duration, and
timeout state. It should return:

- `content`: compact model-visible text
- `meta.stdout`: projected stdout, not raw stdout
- `meta.stderr`: projected stderr, not raw stderr
- `meta.stdout_bytes`
- `meta.stderr_bytes`
- `meta.stdout_truncated`
- `meta.stderr_truncated`
- `meta.output_projection`

Do not put raw/full stdout or stderr into `ToolResult.meta` yet, because the
current runtime serializes the whole `ToolResult` JSON into model context.

- [ ] **Step 3: Preserve useful failure tail**

Use head/tail projection for long output so final traceback and final test
failure lines remain visible.

- [ ] **Step 4: Leave persistent exec output unchanged**

Persistent exec sessions keep their current poll snapshot behavior in this
task. Delta polling, head/tail live buffers, and raw output artifacts are
follow-ups.

- [ ] **Step 5: Verify**

Run:

```bash
cargo test --test run_command
cargo test
```

Expected: PASS.

## Recommended Execution Order

1. Task 1: Process-group cleanup.
2. Task 3: Workspace-scoped absolute paths.
3. Task 4: Command output projection.
4. Task 2: Codex-like follow-up loop.

Process cleanup and path normalization are correctness fixes. Output projection
reduces prompt pressure before the default tool-round cap is removed. The
Codex-like follow-up loop should land after the execution and output layers are
less likely to leak processes or flood model context.
