# Architecture Decision Records

ADRs explain important design choices. They are not module documentation and not file summaries.

Use an ADR when the answer to "why is this designed this way?" should survive future refactors.

## Current ADRs

- [0001-use-rollout-jsonl-as-source-of-truth.md](0001-use-rollout-jsonl-as-source-of-truth.md)
- [0002-use-thread-runtime-actor.md](0002-use-thread-runtime-actor.md)
- [0003-use-live-overlay-for-non-durable-state.md](0003-use-live-overlay-for-non-durable-state.md)
- [0004-use-runtime-thinking-mode-config.md](0004-use-runtime-thinking-mode-config.md)
- [0005-use-process-groups-for-command-cleanup.md](0005-use-process-groups-for-command-cleanup.md)
- [0006-model-turn-completion-as-runtime-lifecycle.md](0006-model-turn-completion-as-runtime-lifecycle.md)
- [0007-allow-workspace-scoped-absolute-paths.md](0007-allow-workspace-scoped-absolute-paths.md)
- [0008-project-tool-output-before-model-context.md](0008-project-tool-output-before-model-context.md)
- [0009-use-project-local-rollout-with-desktop-sqlite-index.md](0009-use-project-local-rollout-with-desktop-sqlite-index.md)

## Template

Use [TEMPLATE.md](TEMPLATE.md) for new decisions.

## ADR Granularity

Write one ADR per decision, not one ADR per module or file.

Good ADR topics:

- durable storage format
- actor/runtime ownership
- approval model
- public protocol versioning
- tool side-effect metadata

Poor ADR topics:

- "what does this file do"
- "list all functions in module"
- "notes from reading code"
