#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let command = exagent::cli::parse_cli_command(std::env::args().skip(1).collect::<Vec<_>>())?;

    if let exagent::cli::CliCommand::Api { bind_addr } = &command {
        exagent::api::serve(bind_addr.as_deref()).await?;
        return Ok(());
    }

    let service = exagent::app_server::AppServerService::new();
    let output = exagent::cli_adapter::execute_cli_command(&service, command).await?;
    print!("{}", output.stdout);

    Ok(())
}
