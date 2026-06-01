use exagent::config::AgentConfig;

#[test]
fn agent_config_defaults_are_safe_for_phase1() {
    let cfg = AgentConfig::default();
    assert_eq!(cfg.command_timeout_secs, 30);
    assert_eq!(cfg.max_output_bytes, 8 * 1024);
}
