mod catalog;
mod policy;

use serde::{Deserialize, Serialize};

pub use catalog::{
    all_profiles, profile_for_type, render_spawn_agent_type_description, AgentProfile,
};
pub use policy::{AgentToolPolicy, CollaborationToolCapability, WorkspaceToolCapability};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    Explorer,
    Planner,
    Reviewer,
    Worker,
}

impl AgentType {
    pub const ALL: [AgentType; 4] = [
        AgentType::Explorer,
        AgentType::Planner,
        AgentType::Reviewer,
        AgentType::Worker,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Explorer => "explorer",
            Self::Planner => "planner",
            Self::Reviewer => "reviewer",
            Self::Worker => "worker",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        all_profiles, profile_for_type, render_spawn_agent_type_description, AgentToolPolicy,
        AgentType,
    };

    #[test]
    fn agent_type_deserializes_known_values() {
        assert_eq!(
            serde_json::from_str::<AgentType>("\"explorer\"").unwrap(),
            AgentType::Explorer
        );
        assert_eq!(
            serde_json::from_str::<AgentType>("\"planner\"").unwrap(),
            AgentType::Planner
        );
        assert_eq!(
            serde_json::from_str::<AgentType>("\"reviewer\"").unwrap(),
            AgentType::Reviewer
        );
        assert_eq!(
            serde_json::from_str::<AgentType>("\"worker\"").unwrap(),
            AgentType::Worker
        );
    }

    #[test]
    fn catalog_defaults_omitted_type_to_worker() {
        let profile = profile_for_type(None);

        assert_eq!(profile.id, AgentType::Worker);
        assert_eq!(profile.display_name, "Worker");
        assert_eq!(profile.tool_policy, AgentToolPolicy::all());
    }

    #[test]
    fn all_profiles_follow_agent_type_catalog_order() {
        let ids = all_profiles()
            .into_iter()
            .map(|profile| profile.id)
            .collect::<Vec<_>>();

        assert_eq!(ids, AgentType::ALL.to_vec());
    }

    #[test]
    fn read_only_profiles_do_not_allow_writes_or_shell() {
        for agent_type in [AgentType::Explorer, AgentType::Planner, AgentType::Reviewer] {
            let profile = profile_for_type(Some(agent_type));

            assert!(profile.tool_policy.allows("read_file"));
            assert!(profile.tool_policy.allows("search_files"));
            assert!(profile.tool_policy.allows("web_search"));
            assert!(profile.tool_policy.allows("list_agents"));
            assert!(profile.tool_policy.allows("send_message"));
            assert!(profile.tool_policy.allows("wait_agent"));
            assert!(!profile.tool_policy.allows("write_file"));
            assert!(!profile.tool_policy.allows("run_command"));
            assert!(!profile.tool_policy.allows("spawn_agent"));
            assert!(!profile.tool_policy.allows("close_agent"));
        }
    }

    #[test]
    fn reviewer_profile_owns_submit_review_and_clean_context_default() {
        let reviewer = profile_for_type(Some(AgentType::Reviewer));

        assert!(reviewer.tool_policy.allows("submit_review"));
        assert_eq!(
            reviewer.default_fork_turns,
            crate::state::fork_history::ForkTurns::None
        );

        for agent_type in [AgentType::Explorer, AgentType::Planner, AgentType::Worker] {
            let profile = profile_for_type(Some(agent_type));
            assert!(
                !profile.tool_policy.allows("submit_review"),
                "{agent_type:?} must not be able to submit a review"
            );
        }
    }

    #[test]
    fn agent_type_as_str_returns_schema_name() {
        assert_eq!(AgentType::Explorer.as_str(), "explorer");
        assert_eq!(AgentType::Planner.as_str(), "planner");
        assert_eq!(AgentType::Reviewer.as_str(), "reviewer");
        assert_eq!(AgentType::Worker.as_str(), "worker");
    }

    #[test]
    fn spawn_agent_type_description_renders_parent_visible_profile_guidance() {
        let description = render_spawn_agent_type_description();

        assert!(description.contains("Available profiles:"));
        assert!(description.contains("explorer (Explorer):"));
        assert!(description.contains("planner (Planner):"));
        assert!(description.contains("reviewer (Reviewer):"));
        assert!(description.contains("worker (Worker):"));
        assert!(description.contains("When to spawn:"));
        assert!(description.contains("Visible tools: workspace=ReadOnly, collaboration=Basic"));
        assert!(description.contains("Defaults: fork_turns=none, thinking_mode=high"));
        assert!(
            !description.to_lowercase().contains("locked"),
            "schema guidance must not advertise unenforced locking"
        );
    }
}
