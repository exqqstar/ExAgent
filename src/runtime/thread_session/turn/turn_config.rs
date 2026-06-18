use crate::config::{AgentConfig, ThinkingMode};
use crate::resolved::ResolvedModelConfig;
use crate::runtime::agent_profile::{profile_for_type, AgentType};
use crate::runtime::context::AgentRuntimeProfileContext;
use crate::runtime::thread_runtime::ThreadTurnContext;
use crate::runtime::turn_mode::TurnMode;
use crate::session::ThreadSnapshot;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum TurnThinkingModeOverride {
    #[default]
    Inherit,
    Set(ThinkingMode),
    ClearDefault,
}

impl TurnThinkingModeOverride {
    pub(super) fn from_turn_context(turn_context: Option<&ThreadTurnContext>) -> Self {
        let Some(turn_context) = turn_context else {
            return Self::default();
        };
        if let Some(thinking_mode) = turn_context.thinking_mode {
            return Self::Set(thinking_mode);
        }
        if turn_context.clear_thinking_mode {
            return Self::ClearDefault;
        }
        Self::Inherit
    }

    pub(super) fn effective(self, agent_default: Option<ThinkingMode>) -> Option<ThinkingMode> {
        match self {
            Self::Inherit => agent_default,
            Self::Set(thinking_mode) => Some(thinking_mode),
            Self::ClearDefault => None,
        }
    }
}

pub(super) fn agent_profile_context_for_turn(
    snapshot: &ThreadSnapshot,
    turn_mode: TurnMode,
) -> Option<AgentRuntimeProfileContext> {
    let lineage = snapshot.lineage.as_ref();
    let agent_type = effective_profile_agent_type_for_turn(snapshot, turn_mode);
    let agent_role = lineage.and_then(|lineage| lineage.agent_role.clone());
    if agent_type.is_none() && agent_role.is_none() {
        return None;
    }

    let (instructions, response_guidance) = match agent_type {
        Some(agent_type) => {
            let profile = profile_for_type(Some(agent_type));
            (Some(profile.instructions), Some(profile.response_guidance))
        }
        None => (None, None),
    };
    Some(AgentRuntimeProfileContext {
        agent_type,
        agent_role,
        instructions,
        response_guidance,
    })
}

pub(super) fn agent_tool_policy(
    snapshot: &ThreadSnapshot,
    turn_mode: TurnMode,
) -> crate::runtime::agent_profile::AgentToolPolicy {
    match effective_profile_agent_type_for_turn(snapshot, turn_mode) {
        Some(agent_type) => profile_for_type(Some(agent_type)).tool_policy,
        None => crate::runtime::agent_profile::AgentToolPolicy::all(),
    }
}

#[cfg(test)]
pub(super) fn effective_agent_type_for_turn(
    snapshot: &ThreadSnapshot,
    turn_mode: TurnMode,
) -> AgentType {
    effective_profile_agent_type_for_turn(snapshot, turn_mode).unwrap_or(AgentType::Worker)
}

pub(super) fn effective_profile_agent_type_for_turn(
    snapshot: &ThreadSnapshot,
    turn_mode: TurnMode,
) -> Option<AgentType> {
    if matches!(turn_mode, TurnMode::Plan) {
        Some(AgentType::Planner)
    } else {
        snapshot
            .lineage
            .as_ref()
            .and_then(|lineage| lineage.agent_type)
    }
}

pub(super) fn config_for_turn(
    config: &AgentConfig,
    turn_model: Option<&ResolvedModelConfig>,
    turn_thinking_mode: TurnThinkingModeOverride,
    effective_agent_type: Option<AgentType>,
) -> AgentConfig {
    let mut config = config.clone();
    if let Some(model) = turn_model {
        config.model = model.clone();
    }
    let profile_default = effective_agent_type
        .and_then(|agent_type| profile_for_type(Some(agent_type)).default_thinking_mode);
    let inherited_thinking_mode = profile_default.or(config.thinking_mode);
    config.thinking_mode = turn_thinking_mode.effective(inherited_thinking_mode);
    config
}
