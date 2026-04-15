#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let command = exagent::cli::parse_cli_command(std::env::args().skip(1).collect::<Vec<_>>())?;

    if let exagent::cli::CliCommand::Api { bind_addr } = &command {
        exagent::api::serve(bind_addr.as_deref()).await?;
        return Ok(());
    }

    match command {
        exagent::cli::CliCommand::Inspect { parent_session_id } => {
            let config = exagent::config::AgentConfig::default();
            let children = exagent::orchestration::inspect_children(
                &config.workspace_root,
                &parent_session_id,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&exagent::api::InspectResponse { children })?
            );
            return Ok(());
        }
        exagent::cli::CliCommand::Collect { session_id } => {
            let config = exagent::config::AgentConfig::default();
            let session =
                exagent::orchestration::collect_session(&config.workspace_root, &session_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&exagent::api::CollectResponse { session })?
            );
            return Ok(());
        }
        exagent::cli::CliCommand::Run { prompt } => {
            let config = exagent::config::AgentConfig::default();
            let llm = exagent::llm::OpenAiCompatibleLlm::from_env()?;
            let agent =
                exagent::agent::Agent::new(config, Box::new(llm), exagent::default_tool_registry());
            let output = agent.run_with_meta(&prompt).await?;
            println!("{}", output.final_turn.text.unwrap_or_default());
        }
        exagent::cli::CliCommand::Resume { session_id, prompt } => {
            let config = exagent::config::AgentConfig::default();
            let llm = exagent::llm::OpenAiCompatibleLlm::from_env()?;
            let agent =
                exagent::agent::Agent::new(config, Box::new(llm), exagent::default_tool_registry());
            let output = agent.resume(&session_id, &prompt).await?;
            println!("{}", output.final_turn.text.unwrap_or_default());
        }
        exagent::cli::CliCommand::Fork {
            parent_session_id,
            agent_role,
            prompt,
        } => {
            let config = exagent::config::AgentConfig::default();
            let llm = exagent::llm::OpenAiCompatibleLlm::from_env()?;
            let agent =
                exagent::agent::Agent::new(config, Box::new(llm), exagent::default_tool_registry());
            let output = agent
                .fork_session(&parent_session_id, agent_role, &prompt, None)
                .await?;
            println!("{}", output.final_turn.text.unwrap_or_default());
        }
        exagent::cli::CliCommand::Api { .. } => unreachable!("api handled above"),
    };

    Ok(())
}
