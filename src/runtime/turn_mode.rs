use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TurnMode {
    #[default]
    Default,
    Plan,
}

impl TurnMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Plan => "plan",
        }
    }

    pub fn is_default(&self) -> bool {
        matches!(self, Self::Default)
    }

    pub fn prompt_guidance(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Plan => Some(
                "Plan mode is active for this turn.\n\n\
The user is asking for planning, not implementation. Treat requests to build, fix,\n\
change, run migrations, format files, commit, or deploy as requests to plan that\n\
work. Do not mutate workspace files or system state.\n\n\
Use read-only tools to ground the plan in real repository facts. If a required\n\
fact cannot be discovered safely, state the assumption and make the smallest\n\
decision-complete plan around it.\n\n\
Return a proposed plan with objective, assumptions, file map, ordered steps,\n\
verification, and risks. Do not ask the user to choose subagent roles.",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TurnMode;

    #[test]
    fn turn_mode_deserializes_known_values() {
        assert_eq!(
            serde_json::from_str::<TurnMode>("\"default\"").unwrap(),
            TurnMode::Default
        );
        assert_eq!(
            serde_json::from_str::<TurnMode>("\"plan\"").unwrap(),
            TurnMode::Plan
        );
    }

    #[test]
    fn plan_mode_has_prompt_guidance() {
        let guidance = TurnMode::Plan.prompt_guidance().expect("plan guidance");

        assert!(guidance.contains("Plan mode is active"));
        assert!(guidance.contains("planning, not implementation"));
    }

    #[test]
    fn default_mode_has_no_prompt_guidance() {
        assert_eq!(TurnMode::Default.prompt_guidance(), None);
        assert!(TurnMode::Default.is_default());
    }
}
