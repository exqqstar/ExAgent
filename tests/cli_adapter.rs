use std::sync::Mutex;

use exagent::app_server::protocol::{
    AgentRunResponse, BoundaryOp, BoundaryOpResponse, CollectParams, CollectResponse,
    EventsReplayParams, EventsReplayResponse, ForkParams, InspectParams, InspectResponse,
    RunParams, ThreadReadParams, ThreadReadResponse, ThreadResumeParams, ThreadResumeResponse,
    ThreadSpawnChildParams, ThreadSpawnChildResponse, ThreadStartParams, ThreadStartResponse,
    ThreadStatus, TurnInterruptParams, TurnInterruptResponse, TurnStartParams, TurnStartResponse,
};
use exagent::app_server::AppServerBoundary;
use exagent::cli::CliCommand;
use exagent::session::AgentRole;
use exagent::types::{SessionId, TurnId};

struct CliBoundary {
    calls: Mutex<Vec<String>>,
}

impl CliBoundary {
    fn new() -> Self {
        Self {
            calls: Mutex::new(vec![]),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl AppServerBoundary for CliBoundary {
    async fn run(&self, _params: RunParams) -> anyhow::Result<AgentRunResponse> {
        panic!("CLI adapter must not use legacy run");
    }

    async fn fork(&self, _params: ForkParams) -> anyhow::Result<AgentRunResponse> {
        panic!("CLI adapter must not use legacy fork");
    }

    async fn inspect(&self, params: InspectParams) -> anyhow::Result<InspectResponse> {
        self.calls.lock().unwrap().push("inspect".into());
        assert_eq!(params.parent_session_id.as_str(), "session_parent");
        assert_eq!(params.workspace_root, None);

        Ok(InspectResponse { children: vec![] })
    }

    async fn collect(&self, _params: CollectParams) -> anyhow::Result<CollectResponse> {
        panic!("collect is not used in these CLI adapter tests");
    }

    async fn thread_start(&self, params: ThreadStartParams) -> anyhow::Result<ThreadStartResponse> {
        self.calls.lock().unwrap().push("thread_start".into());
        assert_eq!(params.workspace_root, None);
        assert_eq!(params.cwd, None);

        Ok(ThreadStartResponse {
            thread_id: SessionId::new("session_cli"),
            snapshot_path: ".exagent/sessions/session_cli/snapshot.json".into(),
            events_path: ".exagent/sessions/session_cli/events.jsonl".into(),
        })
    }

    async fn thread_read(&self, _params: ThreadReadParams) -> anyhow::Result<ThreadReadResponse> {
        panic!("thread_read is not used in these CLI adapter tests");
    }

    async fn thread_resume(
        &self,
        params: ThreadResumeParams,
    ) -> anyhow::Result<ThreadResumeResponse> {
        self.calls.lock().unwrap().push("thread_resume".into());
        assert_eq!(params.thread_id.as_str(), "session_existing");
        assert_eq!(params.workspace_root, None);
        assert_eq!(params.cwd, None);

        Ok(ThreadResumeResponse {
            thread: ThreadReadResponse {
                thread_id: params.thread_id,
                status: ThreadStatus::Idle,
                active_turn: None,
                latest_turn: None,
                snapshot_path: ".exagent/sessions/session_existing/snapshot.json".into(),
                events_path: ".exagent/sessions/session_existing/events.jsonl".into(),
            },
            ignored_overrides: vec![],
        })
    }

    async fn turn_start(&self, params: TurnStartParams) -> anyhow::Result<TurnStartResponse> {
        self.calls.lock().unwrap().push("turn_start".into());
        let text = match params.prompt.as_str() {
            "new prompt" => "new turn complete",
            "resume prompt" => "resumed turn complete",
            other => panic!("unexpected prompt: {other}"),
        };

        Ok(TurnStartResponse {
            thread_id: params.thread_id.clone(),
            turn_id: TurnId::new("turn_1"),
            output: AgentRunResponse {
                text: Some(text.into()),
                tool_calls: vec![],
                session_id: params.thread_id,
                snapshot_path: ".exagent/sessions/session_cli/snapshot.json".into(),
                events_path: ".exagent/sessions/session_cli/events.jsonl".into(),
            },
        })
    }

    async fn turn_interrupt(
        &self,
        _params: TurnInterruptParams,
    ) -> anyhow::Result<TurnInterruptResponse> {
        panic!("turn_interrupt is not used in these CLI adapter tests");
    }

    async fn thread_spawn_child(
        &self,
        params: ThreadSpawnChildParams,
    ) -> anyhow::Result<ThreadSpawnChildResponse> {
        self.calls.lock().unwrap().push("thread_spawn_child".into());
        assert_eq!(params.parent_thread_id.as_str(), "session_parent");
        assert_eq!(params.agent_role, AgentRole::Spec);
        assert_eq!(params.prompt, "draft spec");
        assert_eq!(params.workspace_root, None);
        assert_eq!(params.cwd, None);

        Ok(ThreadSpawnChildResponse {
            parent_thread_id: params.parent_thread_id,
            child_thread_id: SessionId::new("session_child"),
            agent_role: params.agent_role,
            ignored_overrides: vec![],
            output: AgentRunResponse {
                text: Some("child complete".into()),
                tool_calls: vec![],
                session_id: SessionId::new("session_child"),
                snapshot_path: ".exagent/sessions/session_child/snapshot.json".into(),
                events_path: ".exagent/sessions/session_child/events.jsonl".into(),
            },
        })
    }

    async fn submit_boundary_op(&self, _op: BoundaryOp) -> anyhow::Result<BoundaryOpResponse> {
        panic!("CLI adapter uses typed boundary methods in these tests");
    }

    async fn events_replay(
        &self,
        _params: EventsReplayParams,
    ) -> anyhow::Result<EventsReplayResponse> {
        panic!("events_replay is not used in these CLI adapter tests");
    }
}

#[tokio::test]
async fn cli_run_starts_thread_then_turn_without_legacy_run() {
    let boundary = CliBoundary::new();

    let output = exagent::cli_adapter::execute_cli_command(
        &boundary,
        CliCommand::Run {
            prompt: "new prompt".into(),
        },
    )
    .await
    .unwrap();

    assert_eq!(output.stdout, "new turn complete\n");
    assert_eq!(boundary.calls(), vec!["thread_start", "turn_start"]);
}

#[tokio::test]
async fn cli_resume_reads_thread_then_starts_turn_without_legacy_run() {
    let boundary = CliBoundary::new();

    let output = exagent::cli_adapter::execute_cli_command(
        &boundary,
        CliCommand::Resume {
            session_id: SessionId::new("session_existing"),
            prompt: "resume prompt".into(),
        },
    )
    .await
    .unwrap();

    assert_eq!(output.stdout, "resumed turn complete\n");
    assert_eq!(boundary.calls(), vec!["thread_resume", "turn_start"]);
}

#[tokio::test]
async fn cli_fork_uses_thread_spawn_child_without_legacy_fork() {
    let boundary = CliBoundary::new();

    let output = exagent::cli_adapter::execute_cli_command(
        &boundary,
        CliCommand::Fork {
            parent_session_id: SessionId::new("session_parent"),
            agent_role: AgentRole::Spec,
            prompt: "draft spec".into(),
        },
    )
    .await
    .unwrap();

    assert_eq!(output.stdout, "child complete\n");
    assert_eq!(boundary.calls(), vec!["thread_spawn_child"]);
}
