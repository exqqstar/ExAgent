use exagent::app_server::override_policy::{OverridePolicy, RuntimeOverrides};
use exagent::config::{AgentConfig, PermissionProfile};
use exagent::session::ThreadSnapshot;
use exagent::types::ThreadId;
use tempfile::tempdir;

#[test]
fn merge_thread_start_applies_workspace_and_cwd_request_overrides() {
    let dir = tempdir().unwrap();
    let nested = dir.path().join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let base = AgentConfig::default();

    let config = OverridePolicy::merge_thread_start(
        &base,
        RuntimeOverrides {
            workspace_root: Some(dir.path().to_string_lossy().to_string()),
            cwd: Some("nested".into()),
            permission_profile: None,
        },
    )
    .unwrap();

    assert_eq!(
        config.workspace_root,
        std::fs::canonicalize(dir.path()).unwrap()
    );
    assert_eq!(config.cwd, std::fs::canonicalize(nested).unwrap());
}

#[test]
fn merge_thread_start_accepts_full_access_permission_profile() {
    let dir = tempdir().unwrap();
    let base = AgentConfig::default();

    let config = OverridePolicy::merge_thread_start(
        &base,
        RuntimeOverrides {
            workspace_root: Some(dir.path().to_string_lossy().to_string()),
            cwd: None,
            permission_profile: Some(PermissionProfile::FullAccess),
        },
    )
    .unwrap();

    assert_eq!(config.permission_profile, PermissionProfile::FullAccess);
}

#[test]
fn merge_thread_start_rejects_unsupported_permission_profile() {
    let dir = tempdir().unwrap();
    let base = AgentConfig::default();

    let err = OverridePolicy::merge_thread_start(
        &base,
        RuntimeOverrides {
            workspace_root: Some(dir.path().to_string_lossy().to_string()),
            cwd: None,
            permission_profile: Some(PermissionProfile::Managed),
        },
    )
    .unwrap_err();

    assert!(err
        .to_string()
        .contains("unsupported permission profile: managed"));
}

#[test]
fn merge_thread_read_preserves_base_permission_profile_without_validation() {
    let dir = tempdir().unwrap();
    let base = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        permission_profile: PermissionProfile::Managed,
        ..AgentConfig::default()
    };

    let config = OverridePolicy::merge_thread_read(&base, None).unwrap();

    assert_eq!(
        config.workspace_root,
        std::fs::canonicalize(dir.path()).unwrap()
    );
    assert_eq!(config.cwd, config.workspace_root);
    assert_eq!(config.permission_profile, PermissionProfile::Managed);
}

#[test]
fn merge_turn_start_uses_workspace_lookup_without_mutating_cwd() {
    let dir = tempdir().unwrap();
    let base = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let config = OverridePolicy::merge_turn_start(&base, None).unwrap();

    assert_eq!(
        config.workspace_root,
        std::fs::canonicalize(dir.path()).unwrap()
    );
    assert_eq!(config.cwd, config.workspace_root);
}

#[test]
fn apply_turn_context_rejects_cwd_outside_snapshot_workspace() {
    let dir = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let snapshot = ThreadSnapshot::new_thread(
        ThreadId::new("session_policy"),
        std::fs::canonicalize(dir.path()).unwrap(),
        std::fs::canonicalize(dir.path()).unwrap(),
    );

    let err = OverridePolicy::apply_turn_context(
        &snapshot,
        exagent::app_server::protocol::TurnContextOverrides {
            cwd: Some(outside.path().to_string_lossy().to_string()),
            model: None,
            thinking_mode: None,
            clear_thinking_mode: false,
        },
    )
    .unwrap_err();

    assert!(err
        .to_string()
        .contains("cwd must stay within workspace_root"));
}
