use super::super::{ThreadEventRecorder, ThreadSession};
use crate::agent::Agent;
use crate::runtime::context::ContextManager;
use crate::runtime::goal::runtime::GoalRuntime;
use crate::state::rollout::RolloutStore;

pub(super) struct TurnCtx<'a> {
    pub(super) agent: &'a Agent,
    pub(super) recorder: &'a mut ThreadEventRecorder,
    pub(super) rollout_store: &'a RolloutStore,
    pub(super) context_manager: &'a mut ContextManager,
    pub(super) goal_runtime: Option<&'a GoalRuntime>,
}

impl ThreadSession {
    pub(super) fn turn_ctx(&mut self) -> TurnCtx<'_> {
        let Self {
            agent,
            recorder,
            rollout_store,
            context_manager,
            goal_runtime,
            ..
        } = self;

        TurnCtx {
            agent,
            recorder,
            rollout_store,
            context_manager,
            goal_runtime: goal_runtime.as_deref(),
        }
    }
}
