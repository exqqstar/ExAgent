//! Mode prompt overlays.
//!
//! Each operating mode contributes one overlay block that is appended to the
//! base prompt for a turn or goal. Overlays are authored as markdown assets in
//! `assets/` and compiled into the binary with `include_str!`.
//!
//! There is intentionally no per-model variant axis yet — one overlay per mode.
//! When a model axis is needed, grow [`overlay`] into a resolver keyed by the
//! active model; the call sites stay unchanged.

use crate::app_server::protocol::ThreadGoalMode;

/// A mode that contributes a prompt overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptMode {
    /// Read-only planning turn (`TurnMode::Plan`).
    Plan,
    /// Goal completion guarded by a reviewer (`ThreadGoalMode::Reviewed`).
    GoalReviewed,
    /// Reviewed plus mandatory delegation (`ThreadGoalMode::Intensive`).
    GoalIntensive,
}

const PLAN: &str = include_str!("assets/plan.md");
const REVIEWED: &str = include_str!("assets/reviewed.md");
const INTENSIVE: &str = include_str!("assets/intensive.md");

/// The overlay text for a mode, with any trailing newline trimmed so it embeds
/// cleanly inside a larger prompt block.
pub(crate) fn overlay(mode: PromptMode) -> &'static str {
    match mode {
        PromptMode::Plan => PLAN,
        PromptMode::GoalReviewed => REVIEWED,
        PromptMode::GoalIntensive => INTENSIVE,
    }
    .trim_end()
}

/// The overlay for a goal mode, or `None` for [`ThreadGoalMode::Standard`],
/// which runs on the base goal prompt with no overlay.
pub(crate) fn goal_overlay(mode: ThreadGoalMode) -> Option<&'static str> {
    match mode {
        ThreadGoalMode::Standard => None,
        ThreadGoalMode::Reviewed => Some(overlay(PromptMode::GoalReviewed)),
        ThreadGoalMode::Intensive => Some(overlay(PromptMode::GoalIntensive)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_overlay_states_read_only_contract() {
        let text = overlay(PromptMode::Plan);
        assert!(text.contains("planning, not implementation"));
        assert!(!text.ends_with('\n'));
    }

    #[test]
    fn goal_overlays_carry_reviewer_gate() {
        for mode in [ThreadGoalMode::Reviewed, ThreadGoalMode::Intensive] {
            let text = goal_overlay(mode).expect("overlay present");
            assert!(text.contains("agent_type=reviewer"));
            assert!(text.contains("fork_turns=none"));
        }
    }

    #[test]
    fn standard_goal_has_no_overlay() {
        assert_eq!(goal_overlay(ThreadGoalMode::Standard), None);
    }

    #[test]
    fn intensive_requires_delegation() {
        assert!(overlay(PromptMode::GoalIntensive).contains("delegate"));
    }
}
