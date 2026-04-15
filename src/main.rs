#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let command = exagent::cli::parse_cli_command(std::env::args().skip(1).collect::<Vec<_>>())?;

    if let exagent::cli::CliCommand::Api { bind_addr } = &command {
        exagent::api::serve(bind_addr.as_deref()).await?;
        return Ok(());
    }

    let config = exagent::config::AgentConfig::default();
    let llm = exagent::llm::OpenAiCompatibleLlm::from_env()?;
    let agent = exagent::agent::Agent::new(config, Box::new(llm), exagent::default_tool_registry());

    let output = match command {
        exagent::cli::CliCommand::Run { prompt } => agent.run_with_meta(&prompt).await?,
        exagent::cli::CliCommand::Resume { session_id, prompt } => {
            agent.resume(&session_id, &prompt).await?
        }
        exagent::cli::CliCommand::Fork {
            parent_session_id,
            agent_role,
            prompt,
        } => {
            agent
                .fork_session(&parent_session_id, agent_role, &prompt, None)
                .await?
        }
        exagent::cli::CliCommand::Api { .. } => unreachable!("api handled above"),
    };
    println!("{}", output.final_turn.text.unwrap_or_default());

    Ok(())
}
