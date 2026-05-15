# 04. Code Map

These are the most useful source locations in the Codex repo.

Line numbers may drift over time, but these names are stable enough to search.

## Startup

```text
codex-rs/cli/src/main.rs
  Subcommand::AppServer
```

Purpose:

```text
Parse CLI args, select app-server subcommand, call app-server runtime.
```

```text
codex-rs/app-server/src/lib.rs
  run_main_with_transport(...)
  run_main_with_transport_options(...)
```

Purpose:

```text
Construct EnvironmentManager, ConfigManager, AuthManager, transports,
outbound router, and MessageProcessor.
```

## Transport

```text
codex-rs/app-server/src/transport/mod.rs
  AppServerTransport
  TransportEvent
```

Purpose:

```text
Represent stdio/socket/websocket transports and convert transport activity
into generic connection/message events.
```

## Request Gate

```text
codex-rs/app-server/src/message_processor.rs
  ConnectionSessionState
  handle_client_request(...)
  dispatch_initialized_client_request(...)
```

Purpose:

```text
Handle initialize, record connection capabilities, enforce initialized
state, apply experimental API gate, and dispatch typed requests.
```

## Business Router

```text
codex-rs/app-server/src/codex_message_processor.rs
  CodexMessageProcessor::process_request(...)
```

Purpose:

```text
Large match over ClientRequest enum. Routes thread/start, turn/start,
thread/resume, review/start, config, fs, MCP, plugin APIs, etc.
```

## Thread Start

```text
codex-rs/app-server/src/codex_message_processor.rs
  thread_start(...)
  thread_start_task(...)
  build_thread_config_overrides(...)
```

Purpose:

```text
Translate ThreadStartParams into ConfigOverrides, load final Config,
validate environments/tools, call ThreadManager, attach listener, return
ThreadStartResponse.
```

## Turn Start

```text
codex-rs/app-server/src/codex_message_processor.rs
  turn_start(...)
```

Purpose:

```text
Validate input, load CodexThread, convert V2 UserInput to core UserInput,
validate turn context overrides, build Op, submit to core queue.
```

## Listener and Event Translation

```text
codex-rs/app-server/src/codex_message_processor.rs
  ensure_conversation_listener_task(...)
  ensure_listener_task_running_task(...)
```

Purpose:

```text
Read core EventMsg from CodexThread, update ThreadState, find subscribed
connections, translate events into ServerNotification.
```

```text
codex-rs/app-server/src/thread_state.rs
  ThreadStateManager
  ThreadState
  TurnSummary
```

Purpose:

```text
Track connection/thread subscriptions, active turn summary, pending
interrupts, listener lifecycle, and current turn history.
```

```text
codex-rs/app-server/src/outgoing_message.rs
  OutgoingMessageSender
  ThreadScopedOutgoingMessageSender
```

Purpose:

```text
Send JSON-RPC responses, errors, notifications, and server-initiated client
requests.
```

## Protocol Types

```text
codex-rs/app-server-protocol/src/protocol/v2.rs
  ThreadStartParams
  ThreadStartResponse
  TurnStartParams
  TurnStartResponse
  UserInput
```

Purpose:

```text
Define public API wire types for app-server v2.
```

## Core Runtime

```text
codex-rs/core/src/thread_manager.rs
  ThreadManager
  start_thread_with_options(...)
```

Purpose:

```text
Manage live core threads and create/resume/fork sessions.
```

```text
codex-rs/core/src/codex_thread.rs
  CodexThread
  submit_with_trace(...)
  next_event(...)
  validate_turn_context_overrides(...)
```

Purpose:

```text
Public-ish handle around a core session. Submits Ops and exposes event stream.
```

```text
codex-rs/core/src/session/mod.rs
  Codex::spawn(...)
  Codex::submit_with_trace(...)
  submission_loop(...)
```

Purpose:

```text
Create a core session, start the Op-consuming loop, and enqueue submissions.
```

```text
codex-rs/core/src/session/turn.rs
  run_turn(...)
```

Purpose:

```text
The main model/tool loop for a turn.
```

## Search Queries

Useful ripgrep commands:

```bash
rg -n "async fn thread_start|async fn turn_start|ensure_conversation_listener_task" codex-rs/app-server/src
rg -n "pub async fn submit_with_trace|pub struct Codex|submission_loop" codex-rs/core/src/session
rg -n "pub struct ThreadStartParams|pub struct TurnStartParams|pub enum UserInput" codex-rs/app-server-protocol/src/protocol/v2.rs
```
