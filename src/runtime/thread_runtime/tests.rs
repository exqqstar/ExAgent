use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tempfile::tempdir;
use tokio::sync::{broadcast, Notify};

use crate::agent::Agent;
use crate::app_server::protocol::{ThreadGoalReport, ThreadGoalStatus};
use crate::config::AgentConfig;
use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::index_db::{IndexDb, ProjectUpsert};
use crate::llm::{LlmClient, LlmRequestOptions, MockLlm};
use crate::registry::ToolRegistry;
use crate::resolved::{ResolvedCredential, ResolvedModelConfig};
use crate::runtime::goal::runtime::{GoalRuntime, GoalRuntimeEffect};
use crate::runtime::subagent::InterAgentCommunication;
use crate::runtime::turn_mode::TurnMode;
use crate::state::rollout::{rollout_paths, RolloutItem, RolloutStore, ThreadMeta};
use crate::tools::ToolSpec;
use crate::types::{
    AssistantTurn, ConversationMessage, LlmCompletion, ThreadId, TokenUsage, ToolCall, TurnId,
};

use super::actor::PENDING_MAIL_TURN_PROMPT;
use super::op::ThreadOp;

struct BlockingFirstLlm {
    calls: AtomicUsize,
    started: Arc<Notify>,
    release: Arc<Notify>,
}

struct PromptRecordingLlm {
    prompts: Arc<Mutex<Vec<String>>>,
}

struct RestoreGateProbe {
    restoring: Arc<std::sync::atomic::AtomicBool>,
    attempts: AtomicUsize,
}

struct RestoreGatePermit;

impl WorkspaceRuntimeOpGate for RestoreGateProbe {
    fn begin_runtime_op(
        &self,
        _workspace_root: &std::path::Path,
    ) -> Result<WorkspaceRuntimeOpPermit> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        if self.restoring.load(Ordering::SeqCst) {
            return Err(anyhow!("checkpoint restore is in progress"));
        }
        Ok(Box::new(RestoreGatePermit))
    }
}

#[async_trait]
impl LlmClient for BlockingFirstLlm {
    async fn complete(
        &self,
        _messages: &[ConversationMessage],
        _tools: &[ToolSpec],
        _options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
        if call_index == 0 {
            self.started.notify_one();
            self.release.notified().await;
        }
        Ok(AssistantTurn {
            text: Some(format!("turn {} complete", call_index + 1)),
            tool_calls: vec![],
            reasoning: vec![],
        }
        .into_completion())
    }
}

#[async_trait]
impl LlmClient for PromptRecordingLlm {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        _tools: &[ToolSpec],
        _options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        self.prompts
            .lock()
            .expect("prompts")
            .push(serde_json::to_string(messages).expect("serialize prompt"));
        Ok(AssistantTurn {
            text: Some("processed mail".to_string()),
            tool_calls: vec![],
            reasoning: vec![],
        }
        .into_completion())
    }
}

fn write_rollout_meta(config: &AgentConfig, thread_id: &ThreadId) {
    let rollout_paths = rollout_paths(&config.workspace_root, thread_id);
    RolloutStore::new(rollout_paths.rollout_path)
        .append_items_blocking(&[RolloutItem::ThreadMeta(ThreadMeta {
            thread_id: thread_id.clone(),
            workspace_root: config.workspace_root.clone(),
            initial_cwd: config.cwd.clone(),
            permission_profile: crate::config::PermissionProfile::FullAccess,
            thread_source: Default::default(),
            lineage: None,
            created_at: "2026-06-05T00:00:00Z".to_string(),
        })])
        .expect("write rollout meta");
}

fn blocking_first_runtime(
    thread_id: ThreadId,
    config: AgentConfig,
    started: Arc<Notify>,
    release: Arc<Notify>,
) -> Arc<ThreadRuntime> {
    ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id,
        config,
        blocking_agent_factory(started, release),
    ))
    .expect("spawn runtime")
}

fn blocking_agent_factory(started: Arc<Notify>, release: Arc<Notify>) -> AgentFactory {
    Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(BlockingFirstLlm {
                calls: AtomicUsize::new(0),
                started: started.clone(),
                release: release.clone(),
            }),
            ToolRegistry::new(),
        ))
    })
}

async fn wait_for_turn_completed(events: &mut broadcast::Receiver<RuntimeEvent>, turn_id: &TurnId) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.expect("runtime event");
            if event.turn_id.as_ref() == Some(turn_id)
                && matches!(event.kind, crate::events::RuntimeEventKind::TurnCompleted)
            {
                return;
            }
        }
    })
    .await
    .expect("turn completed event");
}

async fn wait_for_runtime_error(
    events: &mut broadcast::Receiver<RuntimeEvent>,
    turn_id: &TurnId,
) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.expect("runtime event");
            if event.turn_id.as_ref() != Some(turn_id) {
                continue;
            }
            if let crate::events::RuntimeEventKind::RuntimeError { message } = event.kind {
                return message;
            }
        }
    })
    .await
    .expect("runtime error event")
}

async fn wait_for_goal_continuation_started(
    events: &mut broadcast::Receiver<RuntimeEvent>,
) -> TurnId {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.expect("runtime event");
            if matches!(
                event.kind,
                RuntimeEventKind::ThreadGoalContinuationStarted { .. }
            ) {
                return event.turn_id.expect("continuation turn id");
            }
        }
    })
    .await
    .expect("goal continuation started")
}

async fn wait_for_turn_interrupted(
    events: &mut broadcast::Receiver<RuntimeEvent>,
    turn_id: &TurnId,
) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.expect("runtime event");
            if event.turn_id.as_ref() == Some(turn_id)
                && matches!(event.kind, RuntimeEventKind::TurnInterrupted)
            {
                return;
            }
        }
    })
    .await
    .expect("turn interrupted event");
}

async fn wait_until_no_active_turn(runtime: &ThreadRuntime) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if runtime.active_turn_id().is_none() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("active turn cleared");
}

async fn wait_for_goal_status(db: &IndexDb, thread_id: &ThreadId, status: ThreadGoalStatus) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let goal = db
                .get_thread_goal(thread_id)
                .await
                .expect("goal lookup")
                .expect("goal exists");
            let current = match goal.status {
                crate::index_db::ThreadGoalStatusRecord::Active => ThreadGoalStatus::Active,
                crate::index_db::ThreadGoalStatusRecord::Paused => ThreadGoalStatus::Paused,
                crate::index_db::ThreadGoalStatusRecord::Blocked => ThreadGoalStatus::Blocked,
                crate::index_db::ThreadGoalStatusRecord::UsageLimited => {
                    ThreadGoalStatus::UsageLimited
                }
                crate::index_db::ThreadGoalStatusRecord::BudgetLimited => {
                    ThreadGoalStatus::BudgetLimited
                }
                crate::index_db::ThreadGoalStatusRecord::Complete => ThreadGoalStatus::Complete,
            };
            if current == status {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("goal reached expected status");
}

async fn wait_for_goal_report(events: &mut broadcast::Receiver<RuntimeEvent>) -> ThreadGoalReport {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.expect("runtime event");
            if let RuntimeEventKind::ThreadGoalReport { report } = event.kind {
                return report;
            }
        }
    })
    .await
    .expect("goal report event")
}

fn usage(total_tokens: i64) -> TokenUsage {
    TokenUsage {
        total_tokens,
        ..TokenUsage::default()
    }
}

#[tokio::test]
async fn active_goal_auto_continues_until_update_goal_complete() {
    let dir = tempdir().expect("tempdir");
    let thread_id = ThreadId::new("thread_goal_auto_continue");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let db = IndexDb::open(dir.path().join("index.sqlite"))
        .await
        .expect("index db");
    let project = db
        .upsert_project(ProjectUpsert {
            name: "Goal Project".to_string(),
            path: dir.path().to_path_buf(),
        })
        .await
        .expect("project");
    sqlx::query(
        r#"
INSERT INTO threads (
  id, project_id, rollout_path, fallback_title, preview, title_source,
  pinned, status, created_at, updated_at
)
VALUES (?, ?, ?, 'Goal thread', 'Goal preview', 'test', 0, 'idle', 1, 1)
            "#,
    )
    .bind(thread_id.as_str())
    .bind(&project.id)
    .bind(format!("/tmp/{}/rollout.jsonl", thread_id.as_str()))
    .execute(db.pool())
    .await
    .expect("thread row");
    db.insert_thread_goal(&thread_id, "finish automatically", None)
        .await
        .expect("insert goal")
        .expect("new goal");

    let completions = vec![
        LlmCompletion {
            turn: AssistantTurn {
                text: Some("made progress".to_string()),
                tool_calls: vec![],
                reasoning: vec![],
            },
            token_usage: Some(usage(10)),
        },
        LlmCompletion {
            turn: AssistantTurn {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "call_update_goal".to_string(),
                    name: "update_goal".to_string(),
                    arguments: serde_json::json!({ "status": "complete" }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            token_usage: Some(usage(20)),
        },
        LlmCompletion {
            turn: AssistantTurn {
                text: Some("goal complete".to_string()),
                tool_calls: vec![],
                reasoning: vec![],
            },
            token_usage: Some(usage(25)),
        },
    ];
    let completions = Arc::new(Mutex::new(Some(completions)));
    let factory: AgentFactory = Arc::new(move |config| {
        let completions = completions
            .lock()
            .expect("completions mutex")
            .take()
            .expect("agent created once");
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new_completions(completions)),
            ToolRegistry::new(),
        ))
    });
    let runtime = ThreadRuntime::spawn(
        ThreadRuntimeOptions::new(thread_id.clone(), config, factory)
            .with_goal_runtime(Arc::new(GoalRuntime::new(db.clone()))),
    )
    .expect("runtime");
    let mut events = runtime.subscribe_events();

    runtime
        .submit_user_input_and_wait("start".to_string(), None)
        .await
        .expect("initial turn");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.expect("runtime event");
            if matches!(
                event.kind,
                RuntimeEventKind::ThreadGoalContinuationStarted { .. }
            ) {
                return;
            }
        }
    })
    .await
    .expect("continuation started");
    wait_for_goal_status(&db, &thread_id, ThreadGoalStatus::Complete).await;
    let report = wait_for_goal_report(&mut events).await;
    assert_eq!(report.tokens_used, 55);
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn goal_continuation_interrupt_records_interrupted_event() {
    let dir = tempdir().expect("tempdir");
    let thread_id = ThreadId::new("thread_goal_continuation_interrupt");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);

    let db = IndexDb::open(dir.path().join("index.sqlite"))
        .await
        .expect("index db");
    let project = db
        .upsert_project(ProjectUpsert {
            name: "Goal Interrupt Project".to_string(),
            path: dir.path().to_path_buf(),
        })
        .await
        .expect("project");
    sqlx::query(
        r#"
INSERT INTO threads (
  id, project_id, rollout_path, fallback_title, preview, title_source,
  pinned, status, created_at, updated_at
)
VALUES (?, ?, ?, 'Goal thread', 'Goal preview', 'test', 0, 'idle', 1, 1)
        "#,
    )
    .bind(thread_id.as_str())
    .bind(&project.id)
    .bind(format!("/tmp/{}/rollout.jsonl", thread_id.as_str()))
    .execute(db.pool())
    .await
    .expect("thread row");
    db.insert_thread_goal(&thread_id, "continue until interrupted", None)
        .await
        .expect("insert goal")
        .expect("new goal");

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let runtime = ThreadRuntime::spawn(
        ThreadRuntimeOptions::new(
            thread_id.clone(),
            config,
            blocking_agent_factory(started.clone(), release.clone()),
        )
        .with_goal_runtime(Arc::new(GoalRuntime::new(db.clone()))),
    )
    .expect("runtime");
    let mut events = runtime.subscribe_events();

    runtime
        .enqueue_goal_runtime_effect(GoalRuntimeEffect::ScheduleContinuation)
        .await
        .expect("queue goal continuation");
    let continuation_turn_id = wait_for_goal_continuation_started(&mut events).await;
    tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
        .await
        .expect("continuation llm call started");

    let interrupted_turn_id = runtime
        .interrupt_active_turn(Some(&continuation_turn_id))
        .await
        .expect("interrupt active goal continuation");
    assert_eq!(interrupted_turn_id, continuation_turn_id);
    wait_for_turn_interrupted(&mut events, &continuation_turn_id).await;
    wait_until_no_active_turn(&runtime).await;

    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn failed_goal_continuation_records_runtime_error_for_replay() {
    let dir = tempdir().expect("tempdir");
    let thread_id = ThreadId::new("thread_failed_goal_continuation_error");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let db = IndexDb::open(dir.path().join("index.sqlite"))
        .await
        .expect("index db");
    let project = db
        .upsert_project(ProjectUpsert {
            name: "Goal Project".to_string(),
            path: dir.path().to_path_buf(),
        })
        .await
        .expect("project");
    sqlx::query(
        r#"
INSERT INTO threads (
  id, project_id, rollout_path, fallback_title, preview, title_source,
  pinned, status, created_at, updated_at
)
VALUES (?, ?, ?, 'Goal thread', 'Goal preview', 'test', 0, 'idle', 1, 1)
            "#,
    )
    .bind(thread_id.as_str())
    .bind(&project.id)
    .bind(format!("/tmp/{}/rollout.jsonl", thread_id.as_str()))
    .execute(db.pool())
    .await
    .expect("thread row");
    db.insert_thread_goal(&thread_id, "continue and fail visibly", None)
        .await
        .expect("insert goal")
        .expect("new goal");

    let factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new(vec![AssistantTurn {
                text: Some("initial progress".to_string()),
                tool_calls: vec![],
                reasoning: vec![],
            }])),
            ToolRegistry::new(),
        ))
    });
    let runtime = ThreadRuntime::spawn(
        ThreadRuntimeOptions::new(thread_id.clone(), config.clone(), factory)
            .with_goal_runtime(Arc::new(GoalRuntime::new(db.clone()))),
    )
    .expect("runtime");
    let mut events = runtime.subscribe_events();

    runtime
        .submit_user_input_and_wait("start".to_string(), None)
        .await
        .expect("initial turn");
    let failed_turn_id = wait_for_goal_continuation_started(&mut events).await;
    let message = wait_for_runtime_error(&mut events, &failed_turn_id).await;
    assert!(message.contains("MockLlm is out of scripted turns"));
    wait_until_no_active_turn(&runtime).await;

    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    let rollout_items = RolloutStore::read_items(&rollout_paths.rollout_path)
        .await
        .expect("read rollout");
    assert!(rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::EventMsg(event)
            if event.turn_id.as_ref() == Some(&failed_turn_id)
                && matches!(event.kind, crate::events::RuntimeEventKind::TurnStarted)
    )));
    assert!(rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::EventMsg(event)
            if event.turn_id.as_ref() == Some(&failed_turn_id)
                && matches!(event.kind, crate::events::RuntimeEventKind::RuntimeError { .. })
    )));

    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn pending_trigger_mail_starts_turn_after_op_completes() {
    let dir = tempdir().expect("tempdir");
    let thread_id = ThreadId::new("thread_pending_trigger_mail_starts_turn");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let prompts_for_factory = prompts.clone();
    let factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(PromptRecordingLlm {
                prompts: prompts_for_factory.clone(),
            }),
            ToolRegistry::new(),
        ))
    });
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config,
        factory,
    ))
    .expect("runtime");
    let mut events = runtime.subscribe_events();

    runtime
        .enqueue_inter_agent_communication(InterAgentCommunication {
            author_thread_id: ThreadId::new("thread_child"),
            author_path: "/root/child".to_string(),
            recipient_thread_id: thread_id.clone(),
            recipient_path: "/root".to_string(),
            other_recipients: Vec::new(),
            content: "trigger child result".to_string(),
            trigger_turn: true,
            source_turn_id: Some(TurnId::new("turn_child")),
            created_at: "2026-06-12T00:00:00Z".to_string(),
        })
        .await
        .expect("enqueue mail");

    let _ = runtime
        .submit_control_and_wait(ThreadOp::Interrupt { turn_id: None })
        .await;
    wait_for_turn_completed(&mut events, &TurnId::new("turn_1")).await;

    let prompts = prompts.lock().expect("prompts");
    let prompt = prompts.first().expect("mail turn prompt");
    assert!(prompt.contains("inter_agent_communication"));
    assert!(prompt.contains("trigger child result"));
    assert!(prompt.contains(PENDING_MAIL_TURN_PROMPT));
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn checkpoint_restore_guard_blocks_goal_continuation_until_later_idle_check() {
    let dir = tempdir().expect("tempdir");
    let thread_id = ThreadId::new("thread_checkpoint_restore_guard_goal_continue");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let db = IndexDb::open(dir.path().join("index.sqlite"))
        .await
        .expect("index db");
    let project = db
        .upsert_project(ProjectUpsert {
            name: "Goal Restore Guard Project".to_string(),
            path: dir.path().to_path_buf(),
        })
        .await
        .expect("project");
    sqlx::query(
        r#"
INSERT INTO threads (
  id, project_id, rollout_path, fallback_title, preview, title_source,
  pinned, status, created_at, updated_at
)
VALUES (?, ?, ?, 'Goal restore guard', 'Goal restore preview', 'test', 0, 'idle', 1, 1)
            "#,
    )
    .bind(thread_id.as_str())
    .bind(&project.id)
    .bind(format!("/tmp/{}/rollout.jsonl", thread_id.as_str()))
    .execute(db.pool())
    .await
    .expect("thread row");
    db.insert_thread_goal(&thread_id, "continue when restore clears", None)
        .await
        .expect("insert goal")
        .expect("new goal");

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let restoring = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let gate = Arc::new(RestoreGateProbe {
        restoring: restoring.clone(),
        attempts: AtomicUsize::new(0),
    });
    let runtime = ThreadRuntime::spawn(
        ThreadRuntimeOptions::new(
            thread_id.clone(),
            config,
            blocking_agent_factory(started.clone(), release.clone()),
        )
        .with_goal_runtime(Arc::new(GoalRuntime::new(db.clone())))
        .with_workspace_runtime_op_gate(gate.clone()),
    )
    .expect("runtime");

    runtime
        .enqueue_goal_runtime_effect(GoalRuntimeEffect::ScheduleContinuation)
        .await
        .expect("queue goal effect");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), started.notified())
            .await
            .is_err(),
        "goal continuation must not start while checkpoint restore guard is active"
    );
    assert_eq!(runtime.active_turn_id(), None);
    assert!(gate.attempts.load(Ordering::SeqCst) > 0);

    restoring.store(false, Ordering::SeqCst);
    runtime
        .enqueue_goal_runtime_effect(GoalRuntimeEffect::ScheduleContinuation)
        .await
        .expect("queue goal effect after restore");
    tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
        .await
        .expect("goal continuation starts after restore guard drops");

    release.notify_one();
    wait_until_no_active_turn(&runtime).await;
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn rejected_busy_submit_does_not_consume_turn_id() {
    let dir = tempdir().expect("tempdir");
    let thread_id = ThreadId::new("thread_rejected_submit_no_burn");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let runtime = blocking_first_runtime(thread_id, config, started.clone(), release.clone());

    let first_runtime = runtime.clone();
    let first = tokio::spawn(async move {
        first_runtime
            .submit_user_input_and_wait("first".to_string(), None)
            .await
    });

    started.notified().await;
    let rejected = runtime
        .submit_user_input("rejected while busy".to_string(), None)
        .await
        .expect_err("busy runtime should reject second submit");
    assert!(rejected.to_string().contains("thread is busy"));

    release.notify_one();
    let first_result = first.await.expect("first task").expect("first turn");
    let ThreadOpResult::UserInput { turn_id, .. } = first_result else {
        panic!("expected user input result");
    };
    assert_eq!(turn_id, TurnId::new("turn_1"));

    let next_result = runtime
        .submit_user_input_and_wait("next accepted".to_string(), None)
        .await
        .expect("next turn");
    let ThreadOpResult::UserInput { turn_id, .. } = next_result else {
        panic!("expected user input result");
    };
    assert_eq!(turn_id, TurnId::new("turn_2"));
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn concurrent_submits_allocate_and_reserve_atomically() {
    let dir = tempdir().expect("tempdir");
    let thread_id = ThreadId::new("thread_concurrent_submit_atomic");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let runtime = blocking_first_runtime(thread_id, config, started.clone(), release.clone());
    let mut events = runtime.subscribe_events();

    let submit_a_runtime = runtime.clone();
    let submit_a = tokio::spawn(async move {
        submit_a_runtime
            .submit_user_input("concurrent a".to_string(), None)
            .await
    });
    let submit_b_runtime = runtime.clone();
    let submit_b = tokio::spawn(async move {
        submit_b_runtime
            .submit_user_input("concurrent b".to_string(), None)
            .await
    });

    let results = [
        submit_a.await.expect("submit a task"),
        submit_b.await.expect("submit b task"),
    ];
    let accepted: Vec<TurnId> = results
        .iter()
        .filter_map(|result| result.as_ref().ok().cloned())
        .collect();
    let rejected = results.iter().filter(|result| result.is_err()).count();
    assert_eq!(accepted, vec![TurnId::new("turn_1")]);
    assert_eq!(rejected, 1);

    started.notified().await;
    release.notify_one();
    wait_for_turn_completed(&mut events, &TurnId::new("turn_1")).await;

    let next_result = runtime
        .submit_user_input_and_wait("after concurrent".to_string(), None)
        .await
        .expect("next turn");
    let ThreadOpResult::UserInput { turn_id, .. } = next_result else {
        panic!("expected user input result");
    };
    assert_eq!(turn_id, TurnId::new("turn_2"));
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn manual_compaction_reservation_rejects_concurrent_submit() {
    let dir = tempdir().expect("tempdir");
    let thread_id = ThreadId::new("thread_manual_compaction_busy");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    RolloutStore::new(rollout_paths.rollout_path)
        .append_items_blocking(&[
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_0"),
                ConversationMessage::user("old user"),
            ),
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_0"),
                ConversationMessage::assistant(Some("old assistant".to_string()), vec![]),
            ),
        ])
        .expect("seed compaction history");
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let runtime =
        blocking_first_runtime(thread_id.clone(), config, started.clone(), release.clone());

    let compact_runtime = runtime.clone();
    let compact = tokio::spawn(async move { compact_runtime.compact_now().await });
    started.notified().await;
    assert_eq!(runtime.active_turn_id(), None);

    let rejected = runtime
        .submit_user_input("rejected while compacting".to_string(), None)
        .await
        .expect_err("manual compaction should reserve the runtime");
    assert!(matches!(
        rejected.downcast_ref::<ThreadRuntimeError>(),
        Some(ThreadRuntimeError::ThreadBusy(busy_thread_id)) if busy_thread_id == &thread_id
    ));

    release.notify_one();
    compact
        .await
        .expect("compaction task")
        .expect("manual compaction");
    wait_until_no_active_turn(&runtime).await;
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn interrupt_during_manual_compaction_is_rejected_without_sentinel() {
    let dir = tempdir().expect("tempdir");
    let thread_id = ThreadId::new("thread_compact_interrupt");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    RolloutStore::new(rollout_paths.rollout_path)
        .append_items_blocking(&[
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_0"),
                ConversationMessage::user("old user"),
            ),
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_0"),
                ConversationMessage::assistant(Some("old assistant".to_string()), vec![]),
            ),
        ])
        .expect("seed compaction history");
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let runtime =
        blocking_first_runtime(thread_id.clone(), config, started.clone(), release.clone());

    let compact_runtime = runtime.clone();
    let compact = tokio::spawn(async move { compact_runtime.compact_now().await });
    started.notified().await;

    let rejected = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        runtime.interrupt_active_turn(None),
    )
    .await
    .expect("interrupt should not hang")
    .expect_err("manual compaction should not be interruptible");
    let message = rejected.to_string();
    assert!(message.contains("active operation is not interruptible"));
    assert!(!message.contains("manual_compaction"));

    release.notify_one();
    compact
        .await
        .expect("compaction task")
        .expect("manual compaction");
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn compact_now_rejects_while_user_turn_running() {
    let dir = tempdir().expect("tempdir");
    let thread_id = ThreadId::new("thread_compact_rejected_busy_turn");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let runtime =
        blocking_first_runtime(thread_id.clone(), config, started.clone(), release.clone());

    let first_runtime = runtime.clone();
    let first = tokio::spawn(async move {
        first_runtime
            .submit_user_input_and_wait("first".to_string(), None)
            .await
    });
    started.notified().await;
    assert_eq!(runtime.active_turn_id(), Some(TurnId::new("turn_1")));

    let rejected = runtime
        .compact_now()
        .await
        .expect_err("running turn should reject manual compaction");
    assert!(matches!(
        rejected.downcast_ref::<ThreadRuntimeError>(),
        Some(ThreadRuntimeError::ThreadBusy(busy_thread_id)) if busy_thread_id == &thread_id
    ));

    release.notify_one();
    first.await.expect("first task").expect("first turn");
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn pre_start_failure_persists_returned_turn_id_before_error() {
    let dir = tempdir().expect("tempdir");
    let thread_id = ThreadId::new("thread_pre_start_failure_persists_id");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let factory_calls = Arc::new(AtomicUsize::new(0));
    let factory_call_counter = factory_calls.clone();
    let factory: AgentFactory = Arc::new(move |config| {
        let call_index = factory_call_counter.fetch_add(1, Ordering::SeqCst);
        if call_index == 1 {
            return Err(anyhow!("agent swap failed before sampling"));
        }
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new(vec![AssistantTurn {
                text: Some("next turn complete".to_string()),
                tool_calls: vec![],
                reasoning: vec![],
            }])),
            ToolRegistry::new(),
        ))
    });
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config.clone(),
        factory,
    ))
    .expect("spawn runtime");
    let mut events = runtime.subscribe_events();
    let override_model = ResolvedModelConfig::from_provider_profile(
        "openai",
        "override-model",
        None,
        ResolvedCredential::None,
        None,
    );

    let failed_turn_id = runtime
        .submit_user_input(
            "use override model".to_string(),
            Some(ThreadTurnContext {
                cwd: None,
                resolved_model: Some(override_model),
                thinking_mode: None,
                clear_thinking_mode: false,
                turn_mode: TurnMode::Default,
            }),
        )
        .await
        .expect("turn id is returned after TurnStarted is persisted");

    assert_eq!(failed_turn_id, TurnId::new("turn_1"));
    let message = wait_for_runtime_error(&mut events, &failed_turn_id).await;
    assert!(message.contains("agent swap failed before sampling"));
    wait_until_no_active_turn(&runtime).await;

    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    let rollout_items = RolloutStore::read_items(&rollout_paths.rollout_path)
        .await
        .expect("read rollout");
    assert!(rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::EventMsg(event)
            if event.turn_id.as_ref() == Some(&failed_turn_id)
                && matches!(event.kind, crate::events::RuntimeEventKind::TurnStarted)
    )));
    assert!(rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::EventMsg(event)
            if event.turn_id.as_ref() == Some(&failed_turn_id)
                && matches!(event.kind, crate::events::RuntimeEventKind::RuntimeError { .. })
    )));

    let next_result = runtime
        .submit_user_input_and_wait("next accepted".to_string(), None)
        .await
        .expect("next turn");
    let ThreadOpResult::UserInput { turn_id, .. } = next_result else {
        panic!("expected user input result");
    };
    assert_eq!(turn_id, TurnId::new("turn_2"));
    runtime.shutdown().await.expect("shutdown");
}
