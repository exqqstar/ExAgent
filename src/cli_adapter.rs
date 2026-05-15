use anyhow::Result;

use crate::app_server::protocol::{
    CollectParams, InspectParams, ThreadResumeParams, ThreadSpawnChildParams, ThreadStartParams,
    TurnStartParams,
};
use crate::app_server::AppServerBoundary;
use crate::cli::CliCommand;

pub struct CliExecutionOutput {
    pub stdout: String,
}

pub async fn execute_cli_command(
    boundary: &dyn AppServerBoundary,
    command: CliCommand,
) -> Result<CliExecutionOutput> {
    let stdout = match command {
        CliCommand::Inspect { parent_session_id } => {
            let response = boundary
                .inspect(InspectParams {
                    parent_session_id,
                    workspace_root: None,
                })
                .await?;
            format!("{}\n", serde_json::to_string_pretty(&response)?)
        }
        CliCommand::Collect { session_id } => {
            let response = boundary
                .collect(CollectParams {
                    session_id,
                    workspace_root: None,
                })
                .await?;
            format!("{}\n", serde_json::to_string_pretty(&response)?)
        }
        CliCommand::Run { prompt } => {
            let thread = boundary
                .thread_start(ThreadStartParams {
                    workspace_root: None,
                    cwd: None,
                })
                .await?;
            let turn = boundary
                .turn_start(TurnStartParams {
                    thread_id: thread.thread_id,
                    prompt,
                    workspace_root: None,
                    turn_context: None,
                })
                .await?;
            format!("{}\n", turn.output.text.unwrap_or_default())
        }
        CliCommand::Resume { session_id, prompt } => {
            let resumed = boundary
                .thread_resume(ThreadResumeParams {
                    thread_id: session_id,
                    workspace_root: None,
                    cwd: None,
                })
                .await?;
            let turn = boundary
                .turn_start(TurnStartParams {
                    thread_id: resumed.thread.thread_id,
                    prompt,
                    workspace_root: None,
                    turn_context: None,
                })
                .await?;
            format!("{}\n", turn.output.text.unwrap_or_default())
        }
        CliCommand::Fork {
            parent_session_id,
            agent_role,
            prompt,
        } => {
            let child = boundary
                .thread_spawn_child(ThreadSpawnChildParams {
                    parent_thread_id: parent_session_id,
                    agent_role,
                    prompt,
                    workspace_root: None,
                    cwd: None,
                    spawned_by_turn_id: None,
                })
                .await?;
            format!("{}\n", child.output.text.unwrap_or_default())
        }
        CliCommand::Api { .. } => unreachable!("api command is handled by process startup"),
    };

    Ok(CliExecutionOutput { stdout })
}
