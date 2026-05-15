# 02. Codex app-server Flow

This is the concrete flow Codex uses around app-server. The names come from
the Codex repo, but the architecture is generally useful.

## 1. Startup

```text
cli/src/main.rs
  -> codex_app_server::run_main_with_transport(...)
  -> run_main_with_transport_options(...)
```

The startup function receives:

```text
arg0_paths
cli_config_overrides
loader_overrides
default_analytics_enabled
transport
session_source
auth
```

Inside the function, app-server builds the real runtime:

```text
EnvironmentManager
ConfigManager
AuthManager
state db
transport acceptors
outbound router
MessageProcessor
ThreadManager
```

Important point: startup arguments are not the runtime. They are used to
construct the runtime.

## 2. Transport

Codex app-server supports several transport shapes:

```text
Stdio       - child process communicates over stdin/stdout JSONL
UnixSocket  - local app-server control socket
WebSocket   - experimental ws://IP:PORT transport
Off         - no local transport
```

Transport emits generic events:

```text
ConnectionOpened
IncomingMessage
ConnectionClosed
```

Transport does not know what `thread/start` or `turn/start` means. It only
knows how to move JSON-RPC messages.

## 3. initialize

Every connection must initialize first.

The client sends:

```json
{
  "id": 1,
  "method": "initialize",
  "params": {
    "clientInfo": {
      "name": "codex_vscode",
      "title": "Codex VS Code Extension",
      "version": "0.1.0"
    },
    "capabilities": {
      "experimentalApi": true
    }
  }
}
```

app-server records:

```text
client name
client version
experimental API capability
notification opt-out list
connection origin
```

This state lives per connection. Non-initialized connections cannot call normal
business APIs.

## 4. Request Dispatch

After initialize, JSON-RPC requests are deserialized into a typed enum:

```text
ClientRequest::ThreadStart { request_id, params }
ClientRequest::TurnStart { request_id, params }
ClientRequest::ThreadResume { request_id, params }
ClientRequest::ReviewStart { request_id, params }
...
```

Then app-server uses a large `match` to route each request to a handler.

This is the API boundary:

```text
Public JSON-RPC API
  -> ClientRequest enum
  -> handler function
  -> core runtime operation
```

## 5. thread/start

`thread/start` creates a long-lived core session.

Flow:

```text
ThreadStartParams
  -> build_thread_config_overrides(...)
  -> ConfigManager::load_with_overrides(...)
  -> ThreadManager::start_thread_with_options(...)
  -> Codex::spawn(...)
  -> Session::new(...)
  -> spawn submission_loop
  -> attach listener
  -> ThreadStartResponse
  -> ThreadStarted notification
```

Important inputs:

```text
model
model_provider
service_tier
cwd
approval_policy
approvals_reviewer
sandbox / permissions
base_instructions
developer_instructions
personality
environments
dynamic_tools
ephemeral
```

Design lesson:

`thread/start` is where a public API request becomes a configured runtime
session.

## 6. turn/start

`turn/start` starts one unit of work inside a thread.

Flow:

```text
TurnStartParams
  -> validate input size
  -> load thread
  -> map public UserInput into core UserInput
  -> build/validate TurnContext overrides
  -> build Op::UserInput or Op::UserInputWithTurnContext
  -> submit_with_trace(...)
  -> send Submission into core tx_sub queue
  -> TurnStartResponse { status: InProgress }
```

Important point:

`turn/start` does not return the final answer. It returns "the turn started."
The actual result arrives through the event stream.

## 7. Event Stream

Thread creation attaches a listener:

```text
ensure_conversation_listener_task(...)
```

The listener loops on:

```text
conversation.next_event()
```

Then it:

```text
updates ThreadState
finds subscribed connection ids
translates core EventMsg into app-server ServerNotification
sends notifications to clients
```

This makes event streaming a primary output channel, not logging.

## 8. Boundary Summary

```text
app-server owns:
  connection lifecycle
  initialize
  request routing
  public API types
  config/request translation
  thread subscriptions
  event translation

core owns:
  session state
  prompt/context construction
  model loop
  tool routing
  permissions enforcement
  sandboxed execution
  event production
```
