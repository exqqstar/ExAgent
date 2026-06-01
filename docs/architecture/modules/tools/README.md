# Tools Module

## Responsibility

`tools` defines and implements LLM-callable capabilities.

## State

- `ToolRegistry.tools`: registered tool implementations.
- Tool executions receive `ToolContext`, which includes config, session/turn ids, exec session manager, and policy manager.

## File Map

- [registry.md](registry.md)
- [read-write-file.md](read-write-file.md)
- [run-command.md](run-command.md)

## Key Rule

Tools should operate through workspace-bounded paths and return structured `ToolResult` values. Runtime owns interpretation of side-effect metadata.

## Follow-Up Questions

- Decide whether tool exposure should stay global/default, or become scoped by workspace, role, policy mode, or request context.
- Review tool schemas/descriptions because the LLM chooses tools from this contract.
- Define allowlist/denylist behavior for higher-risk tools before expanding the registry.
- Tighten validation and recovery around malformed tool arguments and tool errors.
