#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = std::env::args().skip(1);
    let first_arg = args
        .next()
        .expect("usage: cargo run -- '<prompt>' | cargo run -- api [bind_addr]");

    if first_arg == "api" {
        exagent::api::serve(args.next().as_deref()).await?;
        return Ok(());
    }

    let config = exagent::config::AgentConfig::default();
    let llm = exagent::llm::OpenAiCompatibleLlm::from_env()?;
    let agent = exagent::agent::Agent::new(config, Box::new(llm), exagent::default_tool_registry());
    let final_turn = agent.run(&first_arg).await?;
    println!("{}", final_turn.text.unwrap_or_default());

    Ok(())
}
