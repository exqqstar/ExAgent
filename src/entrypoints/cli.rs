use anyhow::{bail, Result};

use crate::types::ThreadId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    Run { prompt: String },
    Resume { thread_id: ThreadId, prompt: String },
    Api { bind_addr: Option<String> },
}

pub fn parse_cli_command(args: Vec<String>) -> Result<CliCommand> {
    let mut args = args.into_iter();
    let first = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: cargo run -- '<prompt>' | cargo run -- api [bind_addr] | cargo run -- resume <thread_id> '<prompt>'"))?;

    if first == "api" {
        return Ok(CliCommand::Api {
            bind_addr: args.next(),
        });
    }

    if first == "resume" {
        let thread_id = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("usage: cargo run -- resume <thread_id> '<prompt>'"))?;
        let prompt = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("usage: cargo run -- resume <thread_id> '<prompt>'"))?;

        if args.next().is_some() {
            bail!("resume accepts exactly 2 arguments after the subcommand");
        }

        return Ok(CliCommand::Resume {
            thread_id: ThreadId::new(thread_id),
            prompt,
        });
    }

    if args.next().is_some() {
        bail!("prompt mode accepts exactly one prompt argument");
    }

    Ok(CliCommand::Run { prompt: first })
}
