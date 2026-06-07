use anyhow::Result;

use crate::app_server::protocol::{
    EventsSubscribeParams, ThreadResumeParams, ThreadStartParams, TurnStartParams,
};
use crate::app_server::AppServerBoundary;
use crate::cli::CliCommand;
use crate::events::RuntimeEventKind;
use crate::types::TurnId;

pub struct CliExecutionOutput {
    pub stdout: String,
}

pub async fn execute_cli_command(
    boundary: &dyn AppServerBoundary,
    command: CliCommand,
) -> Result<CliExecutionOutput> {
    let stdout = match command {
        CliCommand::Run { prompt } => {
            let thread = boundary
                .thread_start(ThreadStartParams {
                    workspace_root: None,
                    cwd: None,
                    permission_profile: None,
                })
                .await?;
            let mut events = boundary
                .events_subscribe(EventsSubscribeParams {
                    thread_id: thread.thread.id.clone(),
                    workspace_root: None,
                    after_event_id: None,
                })
                .await?;
            let turn = boundary
                .turn_start(TurnStartParams {
                    thread_id: thread.thread.id.clone(),
                    prompt,
                    workspace_root: None,
                    turn_mode: Default::default(),
                    turn_context: None,
                })
                .await?;
            format!(
                "{}\n",
                wait_for_final_assistant_text(&mut events, turn.turn.id).await?
            )
        }
        CliCommand::Resume { thread_id, prompt } => {
            let resumed = boundary
                .thread_resume(ThreadResumeParams {
                    thread_id: thread_id,
                    workspace_root: None,
                    cwd: None,
                })
                .await?;
            let mut events = boundary
                .events_subscribe(EventsSubscribeParams {
                    thread_id: resumed.thread.id.clone(),
                    workspace_root: None,
                    after_event_id: None,
                })
                .await?;
            let turn = boundary
                .turn_start(TurnStartParams {
                    thread_id: resumed.thread.id.clone(),
                    prompt,
                    workspace_root: None,
                    turn_mode: Default::default(),
                    turn_context: None,
                })
                .await?;
            format!(
                "{}\n",
                wait_for_final_assistant_text(&mut events, turn.turn.id).await?
            )
        }
        CliCommand::Api { .. } => unreachable!("api command is handled by process startup"),
    };

    Ok(CliExecutionOutput { stdout })
}

async fn wait_for_final_assistant_text(
    events: &mut tokio::sync::broadcast::Receiver<crate::events::RuntimeEvent>,
    turn_id: TurnId,
) -> Result<String> {
    let mut last_text = None;
    loop {
        let event = events.recv().await?;
        if event.turn_id.as_ref() != Some(&turn_id) {
            continue;
        }
        match event.kind {
            RuntimeEventKind::AssistantTurn { turn } => {
                last_text = turn.text;
            }
            RuntimeEventKind::TurnCompleted => {
                return Ok(last_text.unwrap_or_default());
            }
            RuntimeEventKind::RuntimeError { message } => {
                anyhow::bail!(message);
            }
            RuntimeEventKind::TurnInterrupted => {
                anyhow::bail!("turn interrupted");
            }
            _ => {}
        }
    }
}
