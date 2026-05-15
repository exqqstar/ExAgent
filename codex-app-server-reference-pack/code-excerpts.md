# Code Excerpts and Simplified Shapes

These are condensed shapes of the important Codex code. They are not meant to
replace the source files; they show the structural idea.

## Startup Shape

```rust
run_main_with_transport_options(
    arg0_paths,
    cli_config_overrides,
    loader_overrides,
    default_analytics_enabled,
    transport,
    session_source,
    auth,
) {
    let environment_manager = EnvironmentManager::new(...);
    let config_manager = ConfigManager::new(...);
    let config = config_manager.load_latest_config(...).await?;
    let auth_manager = AuthManager::shared_from_config(...).await;

    start_transport_acceptor(transport);

    spawn(outbound_router_task);

    let processor = MessageProcessor::new(MessageProcessorArgs {
        config,
        config_manager,
        environment_manager,
        auth_manager,
        ...
    });

    loop {
        match transport_event {
            ConnectionOpened => register_connection(),
            IncomingMessage => processor.process_request(...),
            ConnectionClosed => cleanup_connection(),
        }
    }
}
```

## initialize Shape

```rust
handle_client_request(request) {
    if request is Initialize {
        if session.initialized() {
            return error("Already initialized");
        }

        session.initialize({
            experimental_api_enabled,
            opted_out_notification_methods,
            app_server_client_name,
            client_version,
        });

        return InitializeResponse {
            user_agent,
            codex_home,
            platform_family,
            platform_os,
        };
    }

    dispatch_initialized_client_request(request);
}
```

## thread/start Shape

```rust
thread_start(params) {
    let overrides = build_thread_config_overrides(params);

    spawn(async move {
        let config = config_manager
            .load_with_overrides(params.config, overrides)
            .await?;

        validate_environments(params.environments)?;
        validate_dynamic_tools(params.dynamic_tools)?;

        let new_thread = thread_manager
            .start_thread_with_options(StartThreadOptions {
                config,
                initial_history,
                dynamic_tools,
                environments,
                ...
            })
            .await?;

        ensure_conversation_listener_task(new_thread.thread_id, connection_id);

        send_response(ThreadStartResponse { thread, ... });
        send_notification(ThreadStarted { thread });
    });
}
```

## turn/start Shape

```rust
turn_start(params) {
    validate_input_limit(params.input)?;

    let thread = load_thread(params.thread_id).await?;

    let input = params.input
        .into_iter()
        .map(UserInput::into_core)
        .collect();

    let has_overrides = params.cwd.is_some()
        || params.model.is_some()
        || params.permissions.is_some()
        || params.approval_policy.is_some();

    if has_overrides {
        thread.validate_turn_context_overrides(...).await?;
    }

    let op = if has_overrides {
        Op::UserInputWithTurnContext {
            items: input,
            cwd,
            model,
            permission_profile,
            ...
        }
    } else {
        Op::UserInput {
            items: input,
            ...
        }
    };

    let turn_id = thread.submit_with_trace(op, trace).await?;

    send_response(TurnStartResponse {
        turn: Turn {
            id: turn_id,
            status: InProgress,
            items: [],
        }
    });
}
```

## submit_with_trace Shape

```rust
submit_with_trace(op, trace) {
    let id = new_uuid();

    let submission = Submission {
        id,
        op,
        trace,
    };

    tx_sub.send(submission).await?;

    return id;
}
```

## Listener Shape

```rust
ensure_listener_task_running(thread) {
    spawn(async move {
        loop {
            select {
                event = thread.next_event() => {
                    thread_state.track_current_turn_event(event);
                    let connections = thread_state_manager
                        .subscribed_connection_ids(thread_id);

                    let notification = translate_event(event);
                    outgoing.send_to_connections(connections, notification);
                }

                command = listener_command_rx.recv() => {
                    handle_listener_command(command);
                }

                unload = unloading_timer => {
                    unload_thread_if_idle();
                }
            }
        }
    });
}
```

## Your Agent Equivalent

```ts
async function startTurn(request: TurnStartRequest) {
  const thread = await threadManager.load(request.threadId)
  const context = await turnContextBuilder.build(thread, request.overrides)
  await turnContextBuilder.validate(context)

  const turn = await store.createTurn({
    threadId: thread.id,
    input: request.input,
    context,
    status: "queued",
  })

  await opQueue.enqueue({
    type: "user_input",
    threadId: thread.id,
    turnId: turn.id,
    input: request.input,
    context,
  })

  return { turnId: turn.id, status: "in_progress" }
}
```
