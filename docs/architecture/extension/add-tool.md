# Add A Tool

## Normal Change Set

1. Add a tool file under `src/tools/`.
2. Add a module export in `src/tools/mod.rs`.
3. Register the tool in `src/lib.rs::default_tool_registry`.
4. Add focused tests for argument parsing, workspace safety, and result metadata.
5. Update [modules/tools/](../modules/tools/README.md).

## Design Checklist

- What is the tool name?
- What JSON schema does the model see?
- Does it need workspace path safety?
- Does it need policy approval?
- Does it produce runtime side effects through `ToolResult.meta`?
- Should its result be persisted only as conversation/tool result, or does it need a new runtime event?

## Runtime Path

```text
LLM ToolCall
 -> ToolCallRuntime
 -> ToolRegistry
 -> Tool implementation
 -> ToolResult
 -> optional ToolEffect
 -> ConversationMessage::tool
 -> RuntimeEventKind::ToolResult
```
