# Transcript Helpers

## Responsibility

`src/state/transcript.rs` contains JSON helpers, session id generation, and compatibility path construction.

## Current Role

Runtime restoration uses rollout paths. The old `.exagent/sessions/<id>/snapshot.json` and `events.jsonl` paths are compatibility fields returned by protocol responses.

## Important Functions

- `append_json_line`
- `write_json`
- `read_json`
- `read_json_lines`
- `new_session_id`
- `session_paths`
