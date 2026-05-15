# Prompt For Your Agent

Use this prompt with the files in this folder.

```text
You are helping me design an agent runtime inspired by Codex app-server.

Read the reference files in this folder first:

1. README.md
2. 01-runtime-architecture.md
3. 02-app-server-flow.md
4. 03-key-types-and-ops.md
5. 04-code-map.md
6. 05-adaptation-guide.md

Focus on design lessons, not copying Codex implementation details.

My goal is to design my own agent with:

- Thread / Turn / Item state model
- TurnContext as part of the task input
- Op queue between API and core runtime
- streaming events as the main output path
- tool runtime with validation and permission checks
- persistence for resume/debugging

Please help me produce:

1. A minimal architecture for my agent.
2. The core TypeScript/Python/Rust interfaces.
3. The request flow for thread/start and turn/start.
4. The event model.
5. The tool execution path.
6. A phased implementation plan.

Do not overbuild. Start with the smallest runtime that preserves the important boundaries:

Client -> API -> TurnContext -> Op Queue -> Core Loop -> Tool Runtime -> Events -> Persistence.
```
