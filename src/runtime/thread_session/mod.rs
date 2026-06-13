pub mod events;
mod inbox;
pub(crate) mod overlay;
pub mod turn;

pub(crate) use events::{LiveEventSink, ThreadEventRecorder};
pub use inbox::ThreadInbox;
pub(crate) use overlay::{ActiveToolInvocation, RuntimeOverlay};

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tokio::sync::{broadcast, oneshot, watch, Notify};

use crate::agent::Agent;
use crate::config::AgentConfig;
use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::policy::PolicyManager;
use crate::runtime::context::ContextManager;
use crate::runtime::forge::review::ReviewStore;
use crate::runtime::goal::{runtime::GoalRuntime, GoalToolApi};
use crate::runtime::subagent::{
    parent_agent_path, terminal_completion_content, AgentControl, AgentTurnTerminalStatus,
    SendMessageRequest,
};
use crate::runtime::thread_runtime::{
    AgentFactory, ThreadOpResult, ThreadRuntimeError, ThreadRuntimeStatus,
};
use crate::session::ThreadSnapshot;
use crate::state::rollout::{
    events_from_rollout_items, rollout_paths, snapshot_from_rollout_items, RolloutItem,
    RolloutStore,
};
use crate::types::{EventId, ThreadId, TurnId};

const DEFAULT_THREAD_EVENT_CHANNEL_CAPACITY: usize = 256;
const DEFAULT_LIVE_EVENT_BUFFER_CAP: usize = 2048;

pub(crate) struct RuntimeInterrupt {
    pub(crate) interrupt_rx: oneshot::Receiver<()>,
    pub(crate) interrupted: Arc<Notify>,
}

pub struct ThreadSessionOptions {
    thread_id: ThreadId,
    config: AgentConfig,
    agent_factory: AgentFactory,
    event_tx: broadcast::Sender<RuntimeEvent>,
    status_tx: watch::Sender<ThreadRuntimeStatus>,
    policy: Arc<PolicyManager>,
    live_event_buffer_cap: usize,
    subagent_control: Option<Arc<AgentControl>>,
    goal_runtime: Option<Arc<GoalRuntime>>,
    forge_review_store: Option<ReviewStore>,
}

impl ThreadSessionOptions {
    pub fn new(thread_id: ThreadId, config: AgentConfig, agent_factory: AgentFactory) -> Self {
        let (event_tx, _) = broadcast::channel(DEFAULT_THREAD_EVENT_CHANNEL_CAPACITY);
        let (status_tx, _) = watch::channel(ThreadRuntimeStatus::Idle);
        Self {
            thread_id,
            config,
            agent_factory,
            event_tx,
            status_tx,
            policy: Arc::new(PolicyManager::default()),
            live_event_buffer_cap: DEFAULT_LIVE_EVENT_BUFFER_CAP,
            subagent_control: None,
            goal_runtime: None,
            forge_review_store: None,
        }
    }

    pub fn with_event_tx(mut self, event_tx: broadcast::Sender<RuntimeEvent>) -> Self {
        self.event_tx = event_tx;
        self
    }

    pub fn with_status_tx(mut self, status_tx: watch::Sender<ThreadRuntimeStatus>) -> Self {
        self.status_tx = status_tx;
        self
    }

    pub fn with_policy(mut self, policy: Arc<PolicyManager>) -> Self {
        self.policy = policy;
        self
    }

    pub fn with_live_event_buffer_cap(mut self, cap: usize) -> Self {
        self.live_event_buffer_cap = cap.max(1);
        self
    }

    pub fn with_subagent_control(mut self, subagent_control: Option<Arc<AgentControl>>) -> Self {
        self.subagent_control = subagent_control;
        self
    }

    pub(crate) fn with_goal_runtime(mut self, goal_runtime: Option<Arc<GoalRuntime>>) -> Self {
        self.goal_runtime = goal_runtime;
        self
    }

    pub(crate) fn with_forge_review_store(mut self, store: Option<ReviewStore>) -> Self {
        self.forge_review_store = store;
        self
    }
}

pub struct ThreadSession {
    thread_id: ThreadId,
    base_config: AgentConfig,
    agent: Agent,
    agent_factory: AgentFactory,
    recorder: ThreadEventRecorder,
    rollout_store: RolloutStore,
    context_manager: ContextManager,
    status_tx: watch::Sender<ThreadRuntimeStatus>,
    live_state: Arc<RwLock<ThreadSessionLiveState>>,
    policy: Arc<PolicyManager>,
    inbox: Arc<ThreadInbox>,
    subagent_control: Option<Arc<AgentControl>>,
    next_turn_index_seed: u64,
    goal_runtime: Option<Arc<GoalRuntime>>,
    forge_review_store: Option<ReviewStore>,
}

#[derive(Debug, Clone)]
pub struct ThreadSessionLiveView {
    pub thread_id: ThreadId,
    pub snapshot: ThreadSnapshot,
    pub(crate) overlay: RuntimeOverlay,
    pub events: Vec<RuntimeEvent>,
    pub status: ThreadRuntimeStatus,
}

/// Shared, lock-protected publication surface for live thread state.
///
/// Readers (thread_read, live_view) take a read lock and clone out what they
/// need. The actor loop holds the only writer: each event emitted by
/// `ThreadEventRecorder` republishes snapshot + events + status atomically
/// behind the write lock, so the publication never lags by more than one
/// event.
#[derive(Debug)]
pub(crate) struct ThreadSessionLiveState {
    pub(crate) snapshot: ThreadSnapshot,
    pub(crate) overlay: RuntimeOverlay,
    pub(crate) events: Vec<RuntimeEvent>,
    pub(crate) status: ThreadRuntimeStatus,
}

/// Marks the thread as stopped from `Drop`, so the runtime loop reports
/// termination even if a handler panics and the explicit end-of-loop path is
/// skipped.
pub(crate) struct ThreadSessionStoppedGuard {
    status_tx: watch::Sender<ThreadRuntimeStatus>,
    live_state: Arc<RwLock<ThreadSessionLiveState>>,
}

impl Drop for ThreadSessionStoppedGuard {
    fn drop(&mut self) {
        if let Ok(mut live_state) = self.live_state.write() {
            live_state.status = ThreadRuntimeStatus::Stopped;
        }
        let _ = self.status_tx.send(ThreadRuntimeStatus::Stopped);
    }
}

impl ThreadSession {
    pub(crate) fn persisted_runtime_events(&self) -> anyhow::Result<Vec<RuntimeEvent>> {
        let rollout_items = RolloutStore::read_items_blocking(self.rollout_store.path())?;
        Ok(events_from_rollout_items(&rollout_items))
    }

    pub fn new(options: ThreadSessionOptions) -> anyhow::Result<Self> {
        let ThreadSessionOptions {
            thread_id,
            config,
            agent_factory,
            event_tx,
            status_tx,
            policy,
            live_event_buffer_cap,
            subagent_control,
            goal_runtime,
            forge_review_store,
        } = options;
        let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
        let rollout_store = RolloutStore::new(rollout_paths.rollout_path);
        let rollout_items = RolloutStore::read_items_blocking(rollout_store.path())?;
        let next_turn_index_seed = next_turn_index_from_rollout_items(&rollout_items);
        let (snapshot, mut events, context_manager) =
            restore_from_rollout(&thread_id, &rollout_items)?;
        let mut runtime_config = config.clone();
        runtime_config.permission_profile = snapshot.permission_profile;
        if !runtime_config.permission_profile.is_supported() {
            return Err(anyhow::anyhow!(
                "unsupported permission profile: {}",
                runtime_config.permission_profile.as_str()
            ));
        }
        let overlay = RuntimeOverlay::from_events(&events);
        let unfinished_turn_id = unfinished_turn_without_external_wait(&events, &overlay);
        let live_event_buffer_cap = live_event_buffer_cap.max(1);
        let next_event_index = next_event_index(&events);
        let overflow = events.len().saturating_sub(live_event_buffer_cap);
        if overflow > 0 {
            events.drain(0..overflow);
        }
        if let Some(control) = subagent_control.as_ref() {
            control.register_thread_from_snapshot(&snapshot);
        }
        let goal_api = goal_runtime
            .as_ref()
            .map(|runtime| Arc::new(GoalToolApi::new(runtime.clone())));
        let session_subagent_control = subagent_control.clone();
        let agent = agent_factory(runtime_config.clone())?
            .with_subagent_control(subagent_control)
            .with_goal_api(goal_api)
            .with_forge_review_store(forge_review_store.clone());
        let inbox = Arc::new(ThreadInbox::new(thread_id.clone()));
        let live_state = Arc::new(RwLock::new(ThreadSessionLiveState {
            snapshot: snapshot.clone(),
            overlay,
            events,
            status: ThreadRuntimeStatus::Idle,
        }));
        let mut recorder = ThreadEventRecorder::new(
            thread_id.clone(),
            rollout_store.clone(),
            event_tx,
            live_state.clone(),
            next_event_index,
            live_event_buffer_cap,
        );
        if let Some(turn_id) = unfinished_turn_id {
            recorder.record(
                &snapshot,
                Some(&turn_id),
                RuntimeEventKind::RuntimeError {
                    message:
                        "Thread runtime resumed with an unfinished turn; marking the turn failed."
                            .to_string(),
                },
            )?;
        }

        Ok(Self {
            thread_id,
            base_config: runtime_config,
            agent,
            agent_factory,
            recorder,
            rollout_store,
            context_manager,
            status_tx,
            live_state,
            policy,
            inbox,
            subagent_control: session_subagent_control,
            next_turn_index_seed,
            goal_runtime,
            forge_review_store,
        })
    }

    pub fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    pub(crate) fn live_state_handle(&self) -> Arc<RwLock<ThreadSessionLiveState>> {
        self.live_state.clone()
    }

    pub(crate) fn inbox_handle(&self) -> Arc<ThreadInbox> {
        self.inbox.clone()
    }

    pub(crate) fn stopped_guard(&self) -> ThreadSessionStoppedGuard {
        ThreadSessionStoppedGuard {
            status_tx: self.status_tx.clone(),
            live_state: self.live_state.clone(),
        }
    }

    pub(crate) fn next_turn_index_seed(&self) -> u64 {
        self.next_turn_index_seed
    }

    pub(crate) fn workspace_root(&self) -> PathBuf {
        self.live_state
            .read()
            .expect("thread session live state rwlock poisoned")
            .snapshot
            .workspace_root
            .clone()
    }

    pub(crate) fn set_status(&self, status: ThreadRuntimeStatus) {
        if let Ok(mut live_state) = self.live_state.write() {
            live_state.status = status;
        }
        let _ = self.status_tx.send(status);
    }

    pub(crate) async fn shutdown(&self) {
        self.agent.shutdown().await;
    }

    pub(crate) async fn notify_parent_of_terminal_turn(
        &self,
        turn_id: &TurnId,
        status: AgentTurnTerminalStatus,
        message: String,
    ) {
        let Some(control) = self.subagent_control.clone() else {
            return;
        };
        let lineage = {
            let state = self
                .live_state
                .read()
                .expect("thread session live state rwlock poisoned");
            state.snapshot.lineage.clone()
        };
        let Some(lineage) = lineage else {
            return;
        };
        let Some(parent_path) = parent_agent_path(&lineage.agent_path) else {
            return;
        };
        let content = terminal_completion_content(&lineage.agent_path, turn_id, status, &message);
        let request = SendMessageRequest {
            author_thread_id: self.thread_id.clone(),
            config: self.base_config.clone(),
            recipient_path: parent_path,
            message: content,
            source_turn_id: Some(turn_id.clone()),
            followup: true,
        };
        if let Err(err) = control.send_message(request).await {
            tracing::debug!(
                error = %err,
                thread_id = %self.thread_id.as_str(),
                turn_id = %turn_id.as_str(),
                "failed to notify parent of subagent terminal turn"
            );
        }
    }

    pub(crate) fn live_view_from_state(
        thread_id: ThreadId,
        state: &Arc<RwLock<ThreadSessionLiveState>>,
    ) -> ThreadSessionLiveView {
        let state = state
            .read()
            .expect("thread session live state rwlock poisoned");
        ThreadSessionLiveView {
            thread_id,
            snapshot: state.snapshot.clone(),
            overlay: state.overlay.clone(),
            events: state.events.clone(),
            status: state.status,
        }
    }

    pub(crate) async fn handle_interrupt(
        &mut self,
        turn_id: Option<TurnId>,
    ) -> anyhow::Result<ThreadOpResult> {
        let (interrupted_turn_id, snapshot) = {
            let mut state = self
                .live_state
                .write()
                .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?;
            if !state.overlay.has_pending_approval() && !state.overlay.has_pending_user_input() {
                return Err(ThreadRuntimeError::TurnRejected {
                    thread_id: self.thread_id.clone(),
                    reason: "thread has no active turn".to_string(),
                }
                .into());
            }

            let latest_turn_id = state
                .events
                .iter()
                .rev()
                .find_map(|event| event.turn_id.clone());
            let interrupted_turn_id = turn_id.or(latest_turn_id.clone()).ok_or_else(|| {
                ThreadRuntimeError::TurnRejected {
                    thread_id: self.thread_id.clone(),
                    reason: "waiting external input has no turn id".to_string(),
                }
            })?;
            if let Some(latest_turn_id) = latest_turn_id {
                if latest_turn_id != interrupted_turn_id {
                    return Err(ThreadRuntimeError::TurnRejected {
                        thread_id: self.thread_id.clone(),
                        reason: format!(
                            "waiting external input turn is {}",
                            latest_turn_id.as_str()
                        ),
                    }
                    .into());
                }
            }

            state.overlay.clear_pending_approvals();
            state.overlay.clear_pending_user_inputs();
            (interrupted_turn_id, state.snapshot.clone())
        };
        self.policy.cancel_pending_for_thread(&self.thread_id).await;
        // append_and_broadcast checkpoints the snapshot atomically with the
        // event, so a separate pre-event checkpoint is no longer needed.
        self.append_and_broadcast_snapshot(
            &snapshot,
            Some(&interrupted_turn_id),
            RuntimeEventKind::TurnInterrupted,
        )?;

        Ok(ThreadOpResult::Interrupted {
            turn_id: interrupted_turn_id,
        })
    }
}

fn restore_from_rollout(
    requested_thread_id: &ThreadId,
    items: &[RolloutItem],
) -> anyhow::Result<(ThreadSnapshot, Vec<RuntimeEvent>, ContextManager)> {
    let mut snapshot = snapshot_from_rollout_items(requested_thread_id, items)?;
    let context_manager = ContextManager::from_rollout_items(items);
    context_manager.sync_snapshot(&mut snapshot);
    let events = events_from_rollout_items(items);

    Ok((snapshot, events, context_manager))
}

fn next_event_index(events: &[RuntimeEvent]) -> usize {
    events
        .iter()
        .filter_map(|event| parse_event_index(&event.event_id))
        .max()
        .unwrap_or(0)
        + 1
}

fn parse_event_index(event_id: &EventId) -> Option<usize> {
    event_id.as_str().strip_prefix("evt_")?.parse().ok()
}

fn next_turn_index_from_rollout_items(items: &[RolloutItem]) -> u64 {
    items
        .iter()
        .flat_map(turn_ids_from_rollout_item)
        .filter_map(parse_turn_index)
        .max()
        .unwrap_or(0)
        + 1
}

fn unfinished_turn_without_external_wait(
    events: &[RuntimeEvent],
    overlay: &RuntimeOverlay,
) -> Option<TurnId> {
    if overlay.has_pending_approval() || overlay.has_pending_user_input() {
        return None;
    }
    let latest_turn_id = events
        .iter()
        .rev()
        .find_map(|event| event.turn_id.clone())?;
    for event in events
        .iter()
        .rev()
        .filter(|event| event.turn_id.as_ref() == Some(&latest_turn_id))
    {
        match &event.kind {
            RuntimeEventKind::TurnCompleted
            | RuntimeEventKind::TurnInterrupted
            | RuntimeEventKind::RuntimeError { .. } => return None,
            RuntimeEventKind::TurnStarted => return Some(latest_turn_id),
            _ => {}
        }
    }
    None
}

fn turn_ids_from_rollout_item(item: &RolloutItem) -> Vec<&TurnId> {
    match item {
        RolloutItem::ResponseItem(response_item) => vec![&response_item.turn_id],
        RolloutItem::TurnContext(context) => vec![&context.turn_id],
        RolloutItem::EventMsg(event) => event.turn_id.as_ref().into_iter().collect(),
        RolloutItem::ThreadMeta(_) | RolloutItem::Compacted(_) => vec![],
    }
}

fn parse_turn_index(turn_id: &TurnId) -> Option<u64> {
    turn_id.as_str().strip_prefix("turn_")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::config::PermissionProfile;
    use crate::events::RuntimeEvent;
    use crate::llm::MockLlm;
    use crate::policy::PolicyManager;
    use crate::registry::ToolRegistry;
    use crate::runtime::thread_runtime::AgentFactory;
    use crate::runtime::turn_mode::TurnMode;
    use crate::session::ApprovalId;
    use crate::session::TurnContextItem;
    use crate::types::{ConversationMessage, EventId};
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn write_rollout_meta(config: &AgentConfig, thread_id: &ThreadId) {
        write_rollout_meta_with_permission_profile(
            config,
            thread_id,
            PermissionProfile::FullAccess,
        );
    }

    fn write_rollout_meta_with_permission_profile(
        config: &AgentConfig,
        thread_id: &ThreadId,
        permission_profile: PermissionProfile,
    ) {
        let snapshot = ThreadSnapshot::new_thread_with_permission_profile(
            thread_id.clone(),
            config.workspace_root.clone(),
            config.cwd.clone(),
            permission_profile,
        );
        let rollout_paths = crate::state::rollout::rollout_paths(&config.workspace_root, thread_id);
        crate::state::rollout::RolloutStore::new(rollout_paths.rollout_path)
            .append_items_blocking(&[crate::state::rollout::RolloutItem::ThreadMeta(
                crate::state::rollout::thread_meta_from_snapshot(&snapshot),
            )])
            .expect("write rollout session meta");
    }

    #[test]
    fn next_turn_index_scans_raw_rollout_turn_ids() {
        let thread_id = ThreadId::new("thread_turn_index_scan");
        let snapshot = ThreadSnapshot::new_thread(
            thread_id.clone(),
            PathBuf::from("/workspace"),
            PathBuf::from("/workspace"),
        );
        let context = TurnContextItem {
            turn_id: TurnId::new("turn_9"),
            workspace_root: PathBuf::from("/workspace"),
            cwd: PathBuf::from("/workspace"),
            model: crate::resolved::ModelRef::new("openai", "mock"),
            policy_mode: crate::policy::PolicyMode::Off,
            permission_profile: PermissionProfile::FullAccess,
            command_timeout_secs: 30,
            max_output_bytes: 1024,
            turn_mode: TurnMode::Default,
            agent_type: None,
            agent_profile_instructions: None,
            agent_response_guidance: None,
            agent_role: None,
            thinking_mode: None,
            current_utc_date: Some("2026-06-05".to_string()),
        };
        let items = vec![
            RolloutItem::ThreadMeta(crate::state::rollout::thread_meta_from_snapshot(&snapshot)),
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_4"),
                ConversationMessage::user("older"),
            ),
            RolloutItem::TurnContext(context),
            RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_1"),
                thread_id,
                turn_id: Some(TurnId::new("turn_12")),
                kind: RuntimeEventKind::TurnStarted,
            }),
        ];

        assert_eq!(next_turn_index_from_rollout_items(&items), 13);
    }

    #[test]
    fn next_turn_index_ignores_non_numeric_turn_ids() {
        let items = vec![RolloutItem::EventMsg(RuntimeEvent {
            event_id: EventId::new("evt_1"),
            thread_id: ThreadId::new("thread_bad_turn_ids"),
            turn_id: Some(TurnId::new("turn_parent_1")),
            kind: RuntimeEventKind::TurnStarted,
        })];

        assert_eq!(next_turn_index_from_rollout_items(&items), 1);
    }

    #[test]
    fn session_load_uses_persisted_permission_profile_for_agent_config() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_load_persisted_permission_profile");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            permission_profile: PermissionProfile::Managed,
            ..AgentConfig::default()
        };

        write_rollout_meta_with_permission_profile(
            &config,
            &thread_id,
            PermissionProfile::FullAccess,
        );

        let observed_profiles = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed_profiles_for_factory = observed_profiles.clone();
        let agent_factory: AgentFactory = Arc::new(move |config| {
            observed_profiles_for_factory
                .lock()
                .expect("lock observed profiles")
                .push(config.permission_profile);
            assert_eq!(config.permission_profile, PermissionProfile::FullAccess);
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![])),
                ToolRegistry::new(),
            ))
        });

        let _session =
            ThreadSession::new(ThreadSessionOptions::new(thread_id, config, agent_factory))
                .expect("persisted supported profile should override unsupported base profile");

        assert_eq!(
            *observed_profiles.lock().expect("lock observed profiles"),
            vec![PermissionProfile::FullAccess]
        );
    }

    #[test]
    fn session_load_rejects_unsupported_persisted_permission_profile() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_load_unsupported_persisted_permission_profile");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };

        write_rollout_meta_with_permission_profile(&config, &thread_id, PermissionProfile::Managed);

        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![])),
                ToolRegistry::new(),
            ))
        });

        let err =
            match ThreadSession::new(ThreadSessionOptions::new(thread_id, config, agent_factory)) {
                Ok(_) => panic!("unsupported persisted profile should prevent runtime load"),
                Err(err) => err,
            };

        assert!(
            err.to_string()
                .contains("unsupported permission profile: managed"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn session_load_marks_unclosed_turn_without_pending_input_failed() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_load_unclosed_turn_failed");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        let snapshot = ThreadSnapshot::new_thread(
            thread_id.clone(),
            config.workspace_root.clone(),
            config.cwd.clone(),
        );
        let turn_id = TurnId::new("turn_1");
        let rollout_paths =
            crate::state::rollout::rollout_paths(&config.workspace_root, &thread_id);
        crate::state::rollout::RolloutStore::new(rollout_paths.rollout_path.clone())
            .append_items_blocking(&[
                RolloutItem::ThreadMeta(crate::state::rollout::thread_meta_from_snapshot(
                    &snapshot,
                )),
                RolloutItem::EventMsg(RuntimeEvent {
                    event_id: EventId::new("evt_1"),
                    thread_id: thread_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    kind: RuntimeEventKind::TurnStarted,
                }),
            ])
            .expect("write rollout");

        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![])),
                ToolRegistry::new(),
            ))
        });

        let session = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config,
            agent_factory,
        ))
        .expect("create thread session");
        let live_view =
            ThreadSession::live_view_from_state(thread_id, &session.live_state_handle());

        assert!(live_view.events.iter().any(|event| matches!(
            &event.kind,
            RuntimeEventKind::RuntimeError { message }
                if event.turn_id.as_ref() == Some(&turn_id)
                    && message.contains("unfinished turn")
        )));
        let rollout_items =
            crate::state::rollout::RolloutStore::read_items_blocking(&rollout_paths.rollout_path)
                .expect("read rollout");
        assert!(rollout_items.iter().any(|item| matches!(
            item,
            RolloutItem::EventMsg(event)
                if event.turn_id.as_ref() == Some(&turn_id)
                    && matches!(event.kind, RuntimeEventKind::RuntimeError { .. })
        )));
    }

    #[test]
    fn session_load_keeps_unclosed_turn_waiting_for_approval() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_load_unclosed_turn_pending_approval");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        let snapshot = ThreadSnapshot::new_thread(
            thread_id.clone(),
            config.workspace_root.clone(),
            config.cwd.clone(),
        );
        let turn_id = TurnId::new("turn_1");
        let approval_id = ApprovalId::new("approval_1");
        let rollout_paths =
            crate::state::rollout::rollout_paths(&config.workspace_root, &thread_id);
        crate::state::rollout::RolloutStore::new(rollout_paths.rollout_path.clone())
            .append_items_blocking(&[
                RolloutItem::ThreadMeta(crate::state::rollout::thread_meta_from_snapshot(
                    &snapshot,
                )),
                RolloutItem::EventMsg(RuntimeEvent {
                    event_id: EventId::new("evt_1"),
                    thread_id: thread_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    kind: RuntimeEventKind::TurnStarted,
                }),
                RolloutItem::EventMsg(RuntimeEvent {
                    event_id: EventId::new("evt_2"),
                    thread_id: thread_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    kind: RuntimeEventKind::ApprovalRequested {
                        approval_id,
                        tool_name: "run_command".to_string(),
                        reason: "approval required".to_string(),
                        checkpoint_id: None,
                        permission_profile: PermissionProfile::FullAccess,
                        filesystem_sandbox: crate::config::default_boundary_none(),
                        network_sandbox: crate::config::default_boundary_none(),
                        env_isolation: crate::config::default_boundary_none(),
                        command: None,
                    },
                }),
            ])
            .expect("write rollout");

        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![])),
                ToolRegistry::new(),
            ))
        });

        let session = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config,
            agent_factory,
        ))
        .expect("create thread session");
        let live_view =
            ThreadSession::live_view_from_state(thread_id, &session.live_state_handle());

        assert!(!live_view
            .events
            .iter()
            .any(|event| matches!(event.kind, RuntimeEventKind::RuntimeError { .. })));
        assert!(live_view.overlay.has_pending_approval());
    }

    #[tokio::test]
    async fn handle_interrupt_cancels_pending_policy_approvals() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_interrupt_policy");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };

        write_rollout_meta(&config, &thread_id);

        let policy = Arc::new(PolicyManager::default());
        let _registered = policy
            .create_command_approval(
                thread_id.clone(),
                "run_command",
                "rm -rf scratch",
                PathBuf::from("/tmp"),
                None,
                false,
                "policy requires review".to_string(),
            )
            .await;
        assert_eq!(
            policy.pending_count_for_thread(&thread_id).await,
            1,
            "precondition: policy holds one pending approval"
        );

        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![])),
                ToolRegistry::new(),
            ))
        });
        let mut session = ThreadSession::new(
            ThreadSessionOptions::new(thread_id.clone(), config, agent_factory)
                .with_policy(policy.clone()),
        )
        .expect("create thread session");
        {
            let mut state = session.live_state.write().expect("write live state");
            state.overlay.apply_approval_requested(
                ApprovalId::new("approval_test_1"),
                EventId::new("approval_evt_test_1"),
                "run_command".to_string(),
                "policy requires review".to_string(),
                None,
                crate::config::PermissionProfile::FullAccess,
                crate::config::default_boundary_none(),
                crate::config::default_boundary_none(),
                crate::config::default_boundary_none(),
            );
        }

        let turn_id = TurnId::new("turn_approval_1");
        let result = session
            .handle_interrupt(Some(turn_id.clone()))
            .await
            .expect("handle_interrupt should succeed when pending approval exists");
        assert!(matches!(
            result,
            ThreadOpResult::Interrupted { turn_id: ref tid } if tid == &turn_id
        ));

        assert_eq!(
            policy.pending_count_for_thread(&thread_id).await,
            0,
            "interrupt must drop the policy-side approval waiter, not just the snapshot copy"
        );
    }

    #[test]
    fn session_load_bounds_live_events_without_reusing_event_ids() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_load_bounded_events");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        let snapshot = ThreadSnapshot::new_thread(
            thread_id.clone(),
            config.workspace_root.clone(),
            config.cwd.clone(),
        );
        let mut rollout_items = vec![crate::state::rollout::RolloutItem::ThreadMeta(
            crate::state::rollout::thread_meta_from_snapshot(&snapshot),
        )];
        for (event_index, turn_index, kind) in [
            (1, 1, RuntimeEventKind::TurnStarted),
            (2, 1, RuntimeEventKind::TurnCompleted),
            (3, 2, RuntimeEventKind::TurnStarted),
            (4, 2, RuntimeEventKind::TurnCompleted),
        ] {
            rollout_items.push(crate::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new(format!("evt_{}", event_index)),
                thread_id: thread_id.clone(),
                turn_id: Some(TurnId::new(format!("turn_{}", turn_index))),
                kind,
            }));
        }
        let rollout_paths =
            crate::state::rollout::rollout_paths(&config.workspace_root, &thread_id);
        crate::state::rollout::RolloutStore::new(rollout_paths.rollout_path.clone())
            .append_items_blocking(&rollout_items)
            .expect("write rollout items");

        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![])),
                ToolRegistry::new(),
            ))
        });
        let mut session = ThreadSession::new(
            ThreadSessionOptions::new(thread_id.clone(), config.clone(), agent_factory)
                .with_live_event_buffer_cap(2),
        )
        .expect("create thread session");

        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
        assert_eq!(live_view.events.len(), 2);
        assert_eq!(live_view.events[0].event_id, EventId::new("evt_3"));
        assert_eq!(live_view.events[1].event_id, EventId::new("evt_4"));

        let event = session
            .append_and_broadcast_snapshot(
                &snapshot,
                Some(&TurnId::new("turn_5")),
                RuntimeEventKind::TurnStarted,
            )
            .expect("record next event");
        assert_eq!(event.event_id, EventId::new("evt_5"));

        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
        assert_eq!(live_view.events.len(), 2);
        assert_eq!(live_view.events[0].event_id, EventId::new("evt_4"));
        assert_eq!(live_view.events[1].event_id, EventId::new("evt_5"));

        let rollout_items =
            crate::state::rollout::RolloutStore::read_items_blocking(&rollout_paths.rollout_path)
                .expect("read rollout items");
        assert!(rollout_items.iter().any(|item| matches!(
            item,
            crate::state::rollout::RolloutItem::EventMsg(event)
                if event.event_id == EventId::new("evt_5")
        )));
    }
}
