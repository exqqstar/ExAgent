#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let command = exagent::cli::parse_cli_command(std::env::args().skip(1).collect::<Vec<_>>())?;

    if let exagent::cli::CliCommand::Api { bind_addr } = &command {
        exagent::api::serve(bind_addr.as_deref()).await?;
        return Ok(());
    }

    let service = exagent::app_server::AppServerService::new();

    match command {
        exagent::cli::CliCommand::Inspect { parent_session_id } => {
            let response = service.inspect(exagent::app_server::protocol::InspectParams {
                parent_session_id,
                workspace_root: None,
            })?;
            println!("{}", serde_json::to_string_pretty(&response)?);
            return Ok(());
        }
        exagent::cli::CliCommand::Collect { session_id } => {
            let response = service.collect(exagent::app_server::protocol::CollectParams {
                session_id,
                workspace_root: None,
            })?;
            println!("{}", serde_json::to_string_pretty(&response)?);
            return Ok(());
        }
        exagent::cli::CliCommand::Run { prompt } => {
            let output = service
                .run(exagent::app_server::protocol::RunParams {
                    prompt,
                    workspace_root: None,
                    cwd: None,
                    session_id: None,
                })
                .await?;
            println!("{}", output.text.unwrap_or_default());
        }
        exagent::cli::CliCommand::Resume { session_id, prompt } => {
            let output = service
                .run(exagent::app_server::protocol::RunParams {
                    prompt,
                    workspace_root: None,
                    cwd: None,
                    session_id: Some(session_id),
                })
                .await?;
            println!("{}", output.text.unwrap_or_default());
        }
        exagent::cli::CliCommand::Fork {
            parent_session_id,
            agent_role,
            prompt,
        } => {
            let output = service
                .fork(exagent::app_server::protocol::ForkParams {
                    parent_session_id,
                    agent_role,
                    prompt,
                    workspace_root: None,
                    cwd: None,
                    spawned_by_turn_id: None,
                })
                .await?;
            println!("{}", output.text.unwrap_or_default());
        }
        exagent::cli::CliCommand::Api { .. } => unreachable!("api handled above"),
    };

    Ok(())
}
