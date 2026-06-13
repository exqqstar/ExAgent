use std::sync::Mutex;

use exagent::app_server::protocol::{
    AgentRunResponse, AgentTreeParams, AgentTreeResponse, ApprovalDecisionParams,
    ApprovalDecisionResponse, BoundaryOp, BoundaryOpResponse, EventsReplayParams,
    EventsReplayResponse, EventsSubscribeParams, OpenQuestionResolveParams,
    OpenQuestionResolveResponse, RunParams, SubmitUserInputParams, SubmitUserInputResponse,
    ThreadCompactParams, ThreadCompactResponse, ThreadReadParams, ThreadReadResponse,
    ThreadResumeParams, ThreadResumeResponse, ThreadStartParams, ThreadStartResponse, ThreadStatus,
    ThreadView, TurnInterruptParams, TurnInterruptResponse, TurnStartParams, TurnStartResponse,
    TurnStatus, TurnView,
};
use exagent::app_server::AppServerBoundary;
use exagent::cli::CliCommand;
use exagent::events::{RuntimeEvent, RuntimeEventKind};
use exagent::types::{AssistantTurn, EventId, ThreadId, TurnId};

struct CliBoundary {
    calls: Mutex<Vec<String>>,
    event_tx: Mutex<Option<tokio::sync::broadcast::Sender<RuntimeEvent>>>,
}

impl CliBoundary {
    fn new() -> Self {
        Self {
            calls: Mutex::new(vec![]),
            event_tx: Mutex::new(None),
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

    async fn thread_start(&self, params: ThreadStartParams) -> anyhow::Result<ThreadStartResponse> {
        self.calls.lock().unwrap().push("thread_start".into());
        assert_eq!(params.workspace_root, None);
        assert_eq!(params.cwd, None);

        Ok(ThreadStartResponse {
            thread: sample_thread_view(ThreadId::new("session_cli")),
        })
    }

    async fn thread_read(&self, _params: ThreadReadParams) -> anyhow::Result<ThreadReadResponse> {
        panic!("thread_read is not used in these CLI adapter tests");
    }

    async fn thread_compact(
        &self,
        _params: ThreadCompactParams,
    ) -> anyhow::Result<ThreadCompactResponse> {
        panic!("thread_compact is not used in these CLI adapter tests");
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
            thread: sample_thread_view(params.thread_id),
            ignored_overrides: vec![],
        })
    }

    async fn agent_tree(&self, _params: AgentTreeParams) -> anyhow::Result<AgentTreeResponse> {
        panic!("agent_tree is not used in these CLI adapter tests");
    }

    async fn turn_start(&self, params: TurnStartParams) -> anyhow::Result<TurnStartResponse> {
        self.calls.lock().unwrap().push("turn_start".into());
        let text = match params.prompt.as_str() {
            "new prompt" => "new turn complete",
            "resume prompt" => "resumed turn complete",
            other => panic!("unexpected prompt: {other}"),
        };
        let event_tx = self
            .event_tx
            .lock()
            .unwrap()
            .as_ref()
            .expect("events_subscribe should be called before turn_start")
            .clone();
        let _ = event_tx.send(RuntimeEvent {
            event_id: EventId::new("evt_1"),
            thread_id: params.thread_id.clone(),
            turn_id: Some(TurnId::new("turn_1")),
            kind: RuntimeEventKind::AssistantTurn {
                turn: AssistantTurn {
                    text: Some(text.into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                },
            },
        });
        let _ = event_tx.send(RuntimeEvent {
            event_id: EventId::new("evt_2"),
            thread_id: params.thread_id.clone(),
            turn_id: Some(TurnId::new("turn_1")),
            kind: RuntimeEventKind::TurnCompleted,
        });

        Ok(TurnStartResponse {
            thread_id: params.thread_id.clone(),
            turn: TurnView {
                id: TurnId::new("turn_1"),
                status: TurnStatus::InProgress,
                items: vec![],
            },
        })
    }

    async fn turn_interrupt(
        &self,
        _params: TurnInterruptParams,
    ) -> anyhow::Result<TurnInterruptResponse> {
        panic!("turn_interrupt is not used in these CLI adapter tests");
    }

    async fn approval_decision(
        &self,
        _params: ApprovalDecisionParams,
    ) -> anyhow::Result<ApprovalDecisionResponse> {
        panic!("approval_decision is not used in these CLI adapter tests");
    }

    async fn submit_user_input(
        &self,
        _params: SubmitUserInputParams,
    ) -> anyhow::Result<SubmitUserInputResponse> {
        panic!("submit_user_input is not used in these CLI adapter tests");
    }

    async fn open_question_resolve(
        &self,
        _params: OpenQuestionResolveParams,
    ) -> anyhow::Result<OpenQuestionResolveResponse> {
        panic!("open_question_resolve is not used in these CLI adapter tests");
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

    async fn events_subscribe(
        &self,
        _params: EventsSubscribeParams,
    ) -> anyhow::Result<tokio::sync::broadcast::Receiver<RuntimeEvent>> {
        self.calls.lock().unwrap().push("events_subscribe".into());
        let (tx, rx) = tokio::sync::broadcast::channel(8);
        *self.event_tx.lock().unwrap() = Some(tx);
        Ok(rx)
    }
}

fn sample_thread_view(id: ThreadId) -> ThreadView {
    ThreadView {
        id,
        status: ThreadStatus::Idle,
        active_turn: None,
        turns: vec![],
        model: None,
        thinking_mode: None,
        goal: None,
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
    assert_eq!(
        boundary.calls(),
        vec!["thread_start", "events_subscribe", "turn_start"]
    );
}

#[tokio::test]
async fn cli_resume_reads_thread_then_starts_turn_without_legacy_run() {
    let boundary = CliBoundary::new();

    let output = exagent::cli_adapter::execute_cli_command(
        &boundary,
        CliCommand::Resume {
            thread_id: ThreadId::new("session_existing"),
            prompt: "resume prompt".into(),
        },
    )
    .await
    .unwrap();

    assert_eq!(output.stdout, "resumed turn complete\n");
    assert_eq!(
        boundary.calls(),
        vec!["thread_resume", "events_subscribe", "turn_start"]
    );
}
