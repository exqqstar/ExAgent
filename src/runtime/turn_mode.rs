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
            Self::Plan => Some(crate::runtime::prompt::overlay(
                crate::runtime::prompt::PromptMode::Plan,
            )),
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

        assert!(guidance.contains("Plan mode"));
        assert!(guidance.contains("planning, not implementation"));
    }

    #[test]
    fn default_mode_has_no_prompt_guidance() {
        assert_eq!(TurnMode::Default.prompt_guidance(), None);
        assert!(TurnMode::Default.is_default());
    }
}
