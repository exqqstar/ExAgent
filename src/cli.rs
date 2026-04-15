use anyhow::{bail, Result};

use crate::types::SessionId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    Run { prompt: String },
    Resume { session_id: SessionId, prompt: String },
    Api { bind_addr: Option<String> },
}

pub fn parse_cli_command(args: Vec<String>) -> Result<CliCommand> {
    let mut args = args.into_iter();
    let first = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: cargo run -- '<prompt>' | cargo run -- api [bind_addr] | cargo run -- resume <session_id> '<prompt>'"))?;

    if first == "api" {
        return Ok(CliCommand::Api {
            bind_addr: args.next(),
        });
    }

    if first == "resume" {
        let session_id = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("usage: cargo run -- resume <session_id> '<prompt>'"))?;
        let prompt = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("usage: cargo run -- resume <session_id> '<prompt>'"))?;

        if args.next().is_some() {
            bail!("resume accepts exactly 2 arguments after the subcommand");
        }

        return Ok(CliCommand::Resume {
            session_id: SessionId::new(session_id),
            prompt,
        });
    }

    if args.next().is_some() {
        bail!("prompt mode accepts exactly one prompt argument");
    }

    Ok(CliCommand::Run { prompt: first })
}
