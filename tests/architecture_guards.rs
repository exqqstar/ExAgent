fn read_thread_session_turn_sources() -> String {
    [
        "src/runtime/thread_session/turn/mod.rs",
        "src/runtime/thread_session/turn/sampling.rs",
        "src/runtime/thread_session/turn/context_start.rs",
        "src/runtime/thread_session/turn/compaction_flow.rs",
        "src/runtime/thread_session/turn/goal_effects.rs",
        "src/runtime/thread_session/turn/external_input.rs",
        "src/runtime/thread_session/turn/recording.rs",
        "src/runtime/thread_session/turn/turn_config.rs",
    ]
    .into_iter()
    .map(|path| std::fs::read_to_string(path).expect("read thread session turn source"))
    .collect::<Vec<_>>()
    .join("\n")
}

#[test]
fn root_library_doctests_stay_disabled_until_examples_exist() {
    let manifest = std::fs::read_to_string("Cargo.toml").expect("read root manifest");
    assert!(
        manifest.contains("[lib]\ndoctest = false"),
        "root library doctests should stay disabled; rustdoc --test currently hangs without any source doctest examples"
    );
}

#[test]
fn agent_does_not_mutate_session_snapshot_directly() {
    let agent = std::fs::read_to_string("src/runtime/agent.rs").expect("read src/runtime/agent.rs");
    for forbidden in [
        "snapshot.conversation.push",
        "snapshot.open_exec_sessions",
        "snapshot.pending_approvals",
        "LiveEventSink",
    ] {
        assert!(
            !agent.contains(forbidden),
            "src/runtime/agent.rs should not contain {forbidden}"
        );
    }
}

#[test]
fn agent_does_not_keep_mutable_working_conversation() {
    let agent = std::fs::read_to_string("src/runtime/agent.rs").expect("read src/runtime/agent.rs");
    assert!(
        !agent.contains("let mut messages ="),
        "Agent should not keep a mutable working conversation"
    );
}

#[test]
fn agent_does_not_execute_tools_directly() {
    let agent = std::fs::read_to_string("src/runtime/agent.rs").expect("read src/runtime/agent.rs");
    assert!(
        !agent.contains(".execute(call"),
        "Agent should not execute tools directly"
    );
    assert!(
        !agent.contains("ToolContext"),
        "Agent should not build ToolContext directly"
    );
}

#[test]
fn agent_does_not_parse_tool_meta() {
    let agent = std::fs::read_to_string("src/runtime/agent.rs").expect("read src/runtime/agent.rs");
    for forbidden in [
        "approval_status",
        "approval_id",
        "exec_session_id",
        "lifecycle",
    ] {
        assert!(
            !agent.contains(forbidden),
            "Agent should not parse tool meta key {forbidden}"
        );
    }
}

#[test]
fn thread_goal_shape_has_no_forge_fields() {
    let source =
        std::fs::read_to_string("src/app_server/protocol.rs").expect("read protocol source");
    let status_body = source
        .split("pub enum ThreadGoalStatus {")
        .nth(1)
        .and_then(|tail| tail.split_once("}\n\n#[derive"))
        .map(|(body, _)| body)
        .expect("ThreadGoalStatus enum body");
    let variants = status_body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.trim_end_matches(','))
        .collect::<Vec<_>>();
    assert_eq!(
        variants,
        vec![
            "Active",
            "Paused",
            "Blocked",
            "UsageLimited",
            "BudgetLimited",
            "Complete"
        ],
        "Forge must not add goal status variants"
    );

    let goal_body = source
        .split("pub struct ThreadGoal {")
        .nth(1)
        .and_then(|tail| tail.split_once("}\n\n#[derive"))
        .map(|(body, _)| body)
        .expect("ThreadGoal struct body");
    let fields = goal_body
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("pub "))
        .filter_map(|line| line.split_once(':').map(|(name, _)| name))
        .collect::<Vec<_>>();
    assert_eq!(
        fields,
        vec![
            "thread_id",
            "goal_id",
            "objective",
            "status",
            "token_budget",
            "tokens_used",
            "time_used_seconds",
            "continuation_suppressed",
            "continuation_suppressed_after_turn_id",
            "created_at_ms",
            "updated_at_ms"
        ],
        "Forge state must stay outside ThreadGoal"
    );
    assert!(!fields.contains(&"mode"));
    assert!(!fields.contains(&"goal_mode"));
    assert!(!fields.contains(&"review"));
    assert!(!fields.contains(&"forge"));
}

#[test]
fn turn_loop_does_not_sample_from_session_snapshot_conversation() {
    let source = read_thread_session_turn_sources();

    assert!(!source.contains("snapshot.conversation.clone()"));
    assert!(!source.contains("ContextManager::for_prompt(snapshot)"));
    assert!(source.contains("let prompt = prompt_for_sampling("));
    assert!(source.contains("context_manager.for_prompt(input_modalities)"));
}

#[test]
fn context_manager_is_stateful_history_owner() {
    let source = std::fs::read_to_string("src/runtime/context.rs").expect("read context manager");

    assert!(source.contains("items: Vec<ConversationMessage>"));
    assert!(source.contains("reference_turn_context: Option<TurnContextItem>"));
}

#[test]
fn rollout_schema_has_codex_style_top_level_items() {
    let source = std::fs::read_to_string("src/state/rollout.rs").expect("read rollout schema");

    for expected in [
        "ThreadMeta",
        "ResponseItem",
        "TurnContext",
        "Compacted",
        "EventMsg",
    ] {
        assert!(source.contains(expected), "missing {expected}");
    }
}

#[test]
fn runtime_overlay_is_explicit_live_only_state() {
    let source =
        std::fs::read_to_string("src/runtime/thread_session/overlay.rs").expect("read overlay");

    assert!(source.contains("struct RuntimeOverlay"));
    assert!(source.contains("open_exec_sessions"));
    assert!(source.contains("pending_approvals"));
}

#[test]
fn rollout_reconstruction_does_not_restore_runtime_overlay_fields() {
    let source = std::fs::read_to_string("src/state/rollout.rs").expect("read rollout schema");

    assert!(source.contains("open_exec_sessions: vec![]"));
    assert!(source.contains("pending_approvals: vec![]"));
}

#[test]
fn run_command_tool_does_not_bypass_rollout_with_transcript_writes() {
    let source =
        std::fs::read_to_string("src/tools/run_command.rs").expect("read run command tool");

    assert!(!source.contains("transcript::append_json_line"));
}

#[test]
fn turn_loop_does_not_mutate_snapshot_live_only_fields() {
    let source = read_thread_session_turn_sources();

    assert!(!source.contains("snapshot.open_exec_sessions"));
    assert!(!source.contains("snapshot.pending_approvals"));
}

#[test]
fn app_server_runtime_state_is_split_from_thread_manager() {
    let loader_source =
        std::fs::read_to_string("src/app_server/runtime_loader.rs").expect("read runtime loader");
    assert!(loader_source.contains("struct RuntimeLoader"));
    assert!(loader_source.contains("loaded_threads"));
    assert!(loader_source.contains("loading_threads"));
    assert!(loader_source.contains("fn ensure_runtime_loaded"));
    assert!(loader_source.contains("fn loading_lock"));
    assert!(loader_source.contains("loaded_threads.insert"));
    assert!(
        !loader_source.contains("use crate::state::rollout"),
        "RuntimeLoader should not know rollout storage details"
    );
    assert!(
        loader_source.contains("fn subagent_control_for_cold_load"),
        "RuntimeSpawner should make cold-load subagent control a required capability"
    );
    assert!(
        !loader_source.contains("read_thread_state_from_storage"),
        "RuntimeLoader should delegate persisted subagent control reconstruction"
    );
    assert!(
        !loader_source.contains("ThreadSpawnEdgeStore"),
        "RuntimeLoader should not know spawn edge storage details"
    );
    assert!(
        !loader_source.contains("subagent_lifecycle"),
        "RuntimeLoader should not have an optional lifecycle fallback branch"
    );

    let manager_source =
        std::fs::read_to_string("src/app_server/thread_manager.rs").expect("read thread manager");
    let manager_production_source = manager_source
        .split("#[cfg(test)]\nmod tests")
        .next()
        .expect("manager production section");
    assert!(
        manager_source.contains("services: Arc<AppServerServices>"),
        "ThreadManager should hold AppServerServices"
    );
    assert!(
        manager_source.contains("thread_processor::thread_start"),
        "ThreadManager should delegate thread requests to thread_processor"
    );
    assert!(
        manager_source.contains("turn_processor::turn_start"),
        "ThreadManager should delegate turn requests to turn_processor"
    );
    assert!(
        manager_source.contains("events_processor::events_replay"),
        "ThreadManager should delegate event requests to events_processor"
    );

    let processor_sources = [
        "src/app_server/request_processors/thread_processor.rs",
        "src/app_server/request_processors/turn_processor.rs",
        "src/app_server/request_processors/events_processor.rs",
    ]
    .into_iter()
    .map(|path| std::fs::read_to_string(path).expect("read request processor"))
    .collect::<Vec<_>>()
    .join("\n");
    assert!(
        processor_sources.contains("services.runtime_loader.resolve_loaded_runtime"),
        "request processors should delegate loaded runtime resolution to RuntimeLoader"
    );
    assert!(
        processor_sources.contains("services.runtime_loader.ensure_runtime_loaded"),
        "request processors should delegate runtime loading to RuntimeLoader"
    );
    assert!(
        !manager_source.contains("loaded_threads:"),
        "ThreadManager should not own loaded runtime cache directly"
    );
    for forbidden in [
        "ThreadRuntime",
        "RuntimeOverlay",
        "read_thread_state_from_storage",
        "thread_exists_in_storage",
        "build_thread_view",
        "latest_turn_state",
        "thread_item_from_event",
        "apply_tool_invocation_event",
    ] {
        assert!(
            !manager_production_source.contains(forbidden),
            "ThreadManager production surface should not contain `{forbidden}`"
        );
    }

    let store_source =
        std::fs::read_to_string("src/app_server/thread_store.rs").expect("read thread store");
    assert!(
        !store_source.contains("use crate::app_server::protocol"),
        "thread_store should reconstruct storage state without protocol DTOs"
    );
}

#[test]
fn tool_call_runtime_owns_selection_not_registry() {
    let source = std::fs::read_to_string("src/runtime/tool/runtime.rs").expect("read tool runtime");

    assert!(
        source.contains("selection: ToolSelection"),
        "ToolCallRuntime should own per-turn ToolSelection"
    );
    assert!(
        !source.contains("registry: ToolRegistry"),
        "ToolCallRuntime should not directly own ToolRegistry after selection refactor"
    );
    assert!(
        !source.contains("self.registry.clone()"),
        "ToolCallRuntime should not clone the registry per visible_specs or tool call"
    );
}

#[test]
fn tool_resolver_is_resolve_only() {
    let source =
        std::fs::read_to_string("src/runtime/tool/resolver.rs").expect("read tool resolver");

    assert!(
        source.contains("struct ToolResolver"),
        "tool/resolver.rs should define ToolResolver"
    );
    for forbidden in [
        "permission_profile",
        "PermissionProfile",
        "agent_tool_policy",
        "AgentToolPolicy",
        "provider_supports_tools",
        "visible_specs",
        "ToolRouteContext",
    ] {
        assert!(
            !source.contains(forbidden),
            "ToolResolver should not contain visibility concern `{forbidden}`"
        );
    }
}

#[test]
fn agent_does_not_assemble_turn_tools_directly() {
    let source = std::fs::read_to_string("src/runtime/agent.rs").expect("read agent");

    assert!(
        source.contains("build_tool_selection"),
        "Agent should delegate turn tool assembly to build_tool_selection"
    );
    for forbidden in [
        "registry.register_handler(SpawnAgentTool",
        "registry.register_handler(ListAgentsTool",
        "registry.register_handler(CloseAgentTool",
        "registry.register_handler(SendMessageTool",
        "registry.register_handler(FollowupTaskTool",
        "registry.register_handler(WaitAgentTool",
        "self.mcp_runtime.handlers().await",
    ] {
        assert!(
            !source.contains(forbidden),
            "Agent should not assemble tools directly with `{forbidden}`"
        );
    }
}
