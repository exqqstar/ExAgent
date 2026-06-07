use exagent::config::AgentConfig;

#[test]
fn agent_config_defaults_are_safe_for_phase1() {
    let cfg = AgentConfig::default();
    assert_eq!(cfg.command_timeout_secs, 30);
    assert_eq!(cfg.max_output_bytes, 8 * 1024);
    assert!(cfg.project_docs_enabled);
    assert_eq!(cfg.project_docs_max_bytes, 64 * 1024);
    assert!(cfg.skills_enabled);
    assert_eq!(cfg.skills_metadata_max_chars, 8 * 1024);
    assert!(cfg.skills_user_roots.is_empty());
}

#[test]
fn agent_config_defaults_to_no_mcp_servers() {
    let cfg = AgentConfig::default();
    assert!(cfg.mcp_servers.is_empty());
}
