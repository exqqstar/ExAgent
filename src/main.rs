#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let prompt = std::env::args()
        .nth(1)
        .expect("usage: cargo run -- '<prompt>'");
    let config = exagent::config::AgentConfig::default();

    let mut registry = exagent::registry::ToolRegistry::new();
    registry.register(exagent::tools::read_file::ReadFileTool);
    registry.register(exagent::tools::write_file::WriteFileTool);
    registry.register(exagent::tools::run_command::RunCommandTool);

    let llm = exagent::llm::OpenAiCompatibleLlm::from_env()?;

    let agent = exagent::agent::Agent::new(config, Box::new(llm), registry);
    let final_turn = agent.run(&prompt).await?;
    println!("{}", final_turn.text.unwrap_or_default());

    Ok(())
}
