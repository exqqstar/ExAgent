# ExAgent Phase 3 Hands-On Filesystem Lab

**Date:** 2026-04-15  
**Status:** Practical learning lab after Phase 3 `P0`, `P1`, and `P2`  
**Audience:** Developers who want to learn the current Phase 3 runtime by running commands and watching `.exagent/sessions/...` change on disk

## 1. What This Lab Teaches

This file is the practical companion to the conceptual docs.

It teaches the current Phase 3 runtime by making you touch the persistence layer directly:

- create a minimal parent and child session fixture
- run the real `inspect` and `collect` surfaces against that fixture
- change `snapshot.json` and watch lifecycle status change
- append `StructuredResultRecorded` events and watch `collect` change

This lab has two parts:

1. `Zero-dependency lab (recommended)`  
   No model calls. You hand-write a tiny `.exagent/sessions` fixture and run the real read-side commands.
2. `Optional live CLI lab`  
   Uses `cargo run -- '<prompt>'` and `cargo run -- fork ...`, so it requires a working `OpenAiCompatibleLlm::from_env()` setup.

## 2. Before You Start

Assume the repo is here:

```bash
export REPO=/Volumes/EXEXEX/ExAgent
```

If your local path differs, change `REPO` first.

The key detail for this lab is:

- `inspect` and `collect` use `AgentConfig::default()`
- `AgentConfig::default()` sets `workspace_root` to the current directory
- so you must run the commands from the lab directory, not from the repo root

That is why all commands below use:

```bash
cargo run --manifest-path "$REPO/Cargo.toml" -- ...
```

This compiles the code from the repo, but uses your current lab directory as the runtime workspace root.

## 3. Lab A: Build A Minimal Session Fixture By Hand

Create a scratch lab:

```bash
export LAB="$(mktemp -d)"
cd "$LAB"
mkdir -p .exagent/sessions/session_parent
mkdir -p .exagent/sessions/session_spec
```

Create the parent snapshot:

```bash
cat > .exagent/sessions/session_parent/snapshot.json <<EOF
{
  "session_id": "session_parent",
  "root_session_id": "session_parent",
  "agent_role": "primary",
  "workspace_root": "$LAB",
  "cwd": "$LAB",
  "conversation": [
    {
      "role": "user",
      "content": "lead phase3"
    }
  ],
  "open_exec_sessions": [],
  "pending_approvals": []
}
EOF
```

Create the child snapshot:

```bash
cat > .exagent/sessions/session_spec/snapshot.json <<EOF
{
  "session_id": "session_spec",
  "parent_session_id": "session_parent",
  "root_session_id": "session_parent",
  "spawned_by_turn_id": "turn_1",
  "agent_role": "spec",
  "workspace_root": "$LAB",
  "cwd": "$LAB",
  "conversation": [
    {
      "role": "user",
      "content": "draft the spec"
    },
    {
      "role": "assistant",
      "content": "spec summary"
    }
  ],
  "open_exec_sessions": [],
  "pending_approvals": []
}
EOF
```

Create the parent event log:

```bash
cat > .exagent/sessions/session_parent/events.jsonl <<'EOF'
{"event_id":"evt_1","session_id":"session_parent","turn_id":"turn_1","kind":{"type":"session_spawned","child_session_id":"session_spec","parent_session_id":"session_parent","agent_role":"spec","spawned_by_turn_id":"turn_1"}}
EOF
```

Create the child event log:

```bash
cat > .exagent/sessions/session_spec/events.jsonl <<'EOF'
{"event_id":"evt_1","session_id":"session_spec","kind":{"type":"session_spawned","child_session_id":"session_spec","parent_session_id":"session_parent","agent_role":"spec","spawned_by_turn_id":"turn_1"}}
{"event_id":"evt_2","session_id":"session_spec","turn_id":"turn_1","kind":{"type":"structured_result_recorded","result":{"schema_version":"phase3_p2/v1","agent_role":"spec","session_id":"session_spec","parent_session_id":"session_parent","source_turn_id":"turn_1","summary":"spec summary","assumptions":["P1 collect exists"],"risks":["scope creep"],"open_questions":["none"],"payload":{"kind":"spec","goals":["add structured contracts"],"non_goals":["no planner"],"acceptance_criteria":["collect returns typed result"],"contract_boundaries":["inspect stays topology-only"]}}}}
EOF
```

Confirm the fixture exists:

```bash
find .exagent -type f | sort
```

At this point you have manually created the exact persistence objects that the runtime normally creates for real sessions:

- `snapshot.json` stores durable current state
- `events.jsonl` stores replayable facts

## 4. Lab B: Run The Real `inspect` Surface

From the lab directory, run:

```bash
cargo run --manifest-path "$REPO/Cargo.toml" -- inspect session_parent
```

What you should see conceptually:

- one child in the `children` array
- `session_id = session_spec`
- `parent_session_id = session_parent`
- `root_session_id = session_parent`
- `agent_role = spec`
- `status = completed`

Why the status is `completed`:

- `pending_approvals` is empty
- `open_exec_sessions` is empty

What this proves:

- `inspect` does not scan the filesystem looking for all child snapshots
- it starts from the parent event log, finds `SessionSpawned`, then reads the child snapshot

To connect that back to code, reread:

- [src/transcript.rs](/Volumes/EXEXEX/ExAgent/src/transcript.rs:107)
- [src/orchestration.rs](/Volumes/EXEXEX/ExAgent/src/orchestration.rs:56)

## 5. Lab C: Run The Real `collect` Surface

From the same lab directory, run:

```bash
cargo run --manifest-path "$REPO/Cargo.toml" -- collect session_spec
```

What you should see conceptually:

- a `child` object that looks like the inspect summary
- a `structured_result` object with `schema_version = phase3_p2/v1`
- a `latest_useful_output` object with:
  - `kind = assistant_text`
  - `content = spec summary`

Why `latest_useful_output` is assistant text here:

- the child snapshot already contains an assistant message
- `collect` prefers assistant text before tool result fallback

What this proves:

- `collect` joins topology/status, typed result, and human-readable output
- `structured_result` and `latest_useful_output` are separate surfaces, not the same field in two shapes

To connect that back to code, reread:

- [src/orchestration.rs](/Volumes/EXEXEX/ExAgent/src/orchestration.rs:66)
- [src/orchestration.rs](/Volumes/EXEXEX/ExAgent/src/orchestration.rs:114)
- [src/transcript.rs](/Volumes/EXEXEX/ExAgent/src/transcript.rs:136)

## 6. Lab D: Watch `collect` Change When You Append Another Structured Result

Append a second typed result event:

```bash
cat >> .exagent/sessions/session_spec/events.jsonl <<'EOF'
{"event_id":"evt_3","session_id":"session_spec","turn_id":"turn_3","kind":{"type":"structured_result_recorded","result":{"schema_version":"phase3_p2/v1","agent_role":"spec","session_id":"session_spec","parent_session_id":"session_parent","source_turn_id":"turn_3","summary":"revised spec summary","assumptions":["P1 collect exists"],"risks":["scope creep"],"open_questions":["none"],"payload":{"kind":"spec","goals":["add structured contracts"],"non_goals":["no planner"],"acceptance_criteria":["collect returns typed result"],"contract_boundaries":["inspect stays topology-only"]}}}}
EOF
```

Now rerun:

```bash
cargo run --manifest-path "$REPO/Cargo.toml" -- collect session_spec
```

What should change:

- `structured_result.summary` should now be `revised spec summary`
- `structured_result.source_turn_id` should now be `turn_3`
- `latest_useful_output.content` should still be `spec summary`

Why:

- `latest_structured_result(...)` scans the child event log backwards
- `latest_useful_output(...)` still reads the latest assistant text from the snapshot conversation

This is one of the most important Phase 3 details:

- typed result freshness is event-driven
- free-form human output freshness is snapshot-driven or tool-result-driven

To connect that back to code, reread:

- [src/transcript.rs](/Volumes/EXEXEX/ExAgent/src/transcript.rs:136)
- [src/orchestration.rs](/Volumes/EXEXEX/ExAgent/src/orchestration.rs:73)

## 7. Lab E: Watch `inspect` Status Change By Editing The Snapshot

Right now the child is `completed`.

### Make It `waiting_approval`

Run:

```bash
python3 - <<'PY'
import json
from pathlib import Path

path = Path(".exagent/sessions/session_spec/snapshot.json")
data = json.loads(path.read_text())
data["pending_approvals"] = [{
    "approval_id": "approval_spec",
    "requested_event_id": "evt_approval",
    "tool_name": "run_command",
    "reason": "needs approval",
    "status": "pending"
}]
data["open_exec_sessions"] = []
path.write_text(json.dumps(data, indent=2))
PY
```

Then rerun:

```bash
cargo run --manifest-path "$REPO/Cargo.toml" -- inspect session_parent
```

Now the child status should be `waiting_approval`.

### Make It `running`

Run:

```bash
python3 - <<'PY'
import json
from pathlib import Path

path = Path(".exagent/sessions/session_spec/snapshot.json")
data = json.loads(path.read_text())
data["pending_approvals"] = []
data["open_exec_sessions"] = [{
    "exec_session_id": "exec_spec_1",
    "command": "cargo test",
    "cwd": data["cwd"],
    "status": "running"
}]
path.write_text(json.dumps(data, indent=2))
PY
```

Then rerun:

```bash
cargo run --manifest-path "$REPO/Cargo.toml" -- inspect session_parent
```

Now the child status should be `running`.

### Reset It Back To `completed`

Run:

```bash
python3 - <<'PY'
import json
from pathlib import Path

path = Path(".exagent/sessions/session_spec/snapshot.json")
data = json.loads(path.read_text())
data["pending_approvals"] = []
data["open_exec_sessions"] = []
path.write_text(json.dumps(data, indent=2))
PY
```

Then rerun `inspect` one more time.

What this proves:

- lifecycle status is derived from snapshot state, not from the event log
- `waiting_approval` wins over `running`
- `completed` is just the fallback when the other two conditions are absent

To connect that back to code, reread:

- [src/orchestration.rs](/Volumes/EXEXEX/ExAgent/src/orchestration.rs:104)
- [tests/orchestration.rs](/Volumes/EXEXEX/ExAgent/tests/orchestration.rs:413)

## 8. Lab F: Observe The Raw Files Directly

At this point, pause and inspect the artifacts directly:

```bash
sed -n '1,200p' .exagent/sessions/session_parent/snapshot.json
sed -n '1,200p' .exagent/sessions/session_parent/events.jsonl
sed -n '1,240p' .exagent/sessions/session_spec/snapshot.json
sed -n '1,240p' .exagent/sessions/session_spec/events.jsonl
```

As you look at them, answer these questions:

1. Which file tells the parent that the child exists?
2. Which file tells `collect` the latest typed result?
3. Which file drives lifecycle status?
4. Which file still contains the free-form assistant summary?

If you can answer those four questions from the files alone, the core Phase 3 persistence model has clicked.

## 9. Optional Live CLI Lab

Only do this if your OpenAI-compatible environment is already configured.

This lab shows the runtime writing these artifacts for real instead of you hand-authoring them.

### Step 1: Create A Clean Live Workspace

```bash
export LIVE_LAB="$(mktemp -d)"
cd "$LIVE_LAB"
```

### Step 2: Run A Root Session

```bash
cargo run --manifest-path "$REPO/Cargo.toml" -- 'lead phase3'
```

Then find the newest session:

```bash
ls -1t .exagent/sessions | head -n 1
```

Save it:

```bash
export PARENT_ID="$(ls -1t .exagent/sessions | head -n 1)"
echo "$PARENT_ID"
```

### Step 3: Fork A Child

```bash
cargo run --manifest-path "$REPO/Cargo.toml" -- fork "$PARENT_ID" spec 'draft the spec'
```

Now inspect the parent:

```bash
cargo run --manifest-path "$REPO/Cargo.toml" -- inspect "$PARENT_ID"
```

From that JSON, note the child id, then save it manually as `CHILD_ID`.

### Step 4: Collect The Child

```bash
cargo run --manifest-path "$REPO/Cargo.toml" -- collect "$CHILD_ID"
```

Now inspect the session directory tree:

```bash
find .exagent/sessions -maxdepth 2 -type f | sort
```

What this live lab adds beyond the zero-dependency lab:

- you see real session ids generated by the runtime
- you see real child snapshot creation
- you can compare the hand-authored fixture with the runtime-authored files

## 10. Map The Lab Back To Tests

If you want to see the same behaviors locked by tests, read these next:

- [tests/orchestration.rs](/Volumes/EXEXEX/ExAgent/tests/orchestration.rs:413)
  `inspect_lists_direct_children_only`
- [tests/orchestration.rs](/Volumes/EXEXEX/ExAgent/tests/orchestration.rs:542)
  `collect_returns_latest_useful_output`
- [tests/structured_contracts.rs](/Volumes/EXEXEX/ExAgent/tests/structured_contracts.rs:87)
  `tool_records_structured_result_for_matching_role`
- [tests/structured_contracts.rs](/Volumes/EXEXEX/ExAgent/tests/structured_contracts.rs:213)
  `collect_returns_structured_result_when_present`
- [tests/resume.rs](/Volumes/EXEXEX/ExAgent/tests/resume.rs:343)
  `collect_returns_latest_structured_result_after_resume`

The lab and the tests are showing the same contracts from two angles:

- the lab shows what it feels like from the outside
- the tests show what is locked in from the inside

## 11. Clean Up

When you are done:

```bash
rm -rf "$LAB"
rm -rf "$LIVE_LAB"
```

## 12. What To Read After This Lab

After the practical lab, the best next steps are:

- reread [Phase 3 Step-By-Step Code Walkthrough](/Volumes/EXEXEX/ExAgent/docs/plans/2026-04-15-exagent-phase3-step-by-step-code-walkthrough.md:1)
  The source code will make much more sense after touching the files directly.
- reread [Phase 3 Runtime Flows And Persistence Guide](/Volumes/EXEXEX/ExAgent/docs/plans/2026-04-15-exagent-phase3-runtime-flows-and-persistence-guide.md:1)
  You now have a concrete picture for every persistence artifact it discusses.
