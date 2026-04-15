use anyhow::{bail, Result};

use crate::session::AgentRole;
use crate::types::SessionId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    Run {
        prompt: String,
    },
    Inspect {
        parent_session_id: SessionId,
    },
    Collect {
        session_id: SessionId,
    },
    Resume {
        session_id: SessionId,
        prompt: String,
    },
    Fork {
        parent_session_id: SessionId,
        agent_role: AgentRole,
        prompt: String,
    },
    Api {
        bind_addr: Option<String>,
    },
}

pub fn parse_cli_command(args: Vec<String>) -> Result<CliCommand> {
    let mut args = args.into_iter();
    let first = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: cargo run -- '<prompt>' | cargo run -- api [bind_addr] | cargo run -- inspect <parent_session_id> | cargo run -- collect <session_id> | cargo run -- resume <session_id> '<prompt>' | cargo run -- fork <parent_session_id> <agent_role> '<prompt>'"))?;

    if first == "api" {
        return Ok(CliCommand::Api {
            bind_addr: args.next(),
        });
    }

    if first == "inspect" {
        let parent_session_id = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("usage: cargo run -- inspect <parent_session_id>"))?;

        if args.next().is_some() {
            bail!("inspect accepts exactly 1 argument after the subcommand");
        }

        return Ok(CliCommand::Inspect {
            parent_session_id: SessionId::new(parent_session_id),
        });
    }

    if first == "collect" {
        let session_id = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("usage: cargo run -- collect <session_id>"))?;

        if args.next().is_some() {
            bail!("collect accepts exactly 1 argument after the subcommand");
        }

        return Ok(CliCommand::Collect {
            session_id: SessionId::new(session_id),
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

    if first == "fork" {
        let parent_session_id = args.next().ok_or_else(|| {
            anyhow::anyhow!("usage: cargo run -- fork <parent_session_id> <agent_role> '<prompt>'")
        })?;
        let agent_role = args.next().ok_or_else(|| {
            anyhow::anyhow!("usage: cargo run -- fork <parent_session_id> <agent_role> '<prompt>'")
        })?;
        let prompt = args.next().ok_or_else(|| {
            anyhow::anyhow!("usage: cargo run -- fork <parent_session_id> <agent_role> '<prompt>'")
        })?;

        if args.next().is_some() {
            bail!("fork accepts exactly 3 arguments after the subcommand");
        }

        return Ok(CliCommand::Fork {
            parent_session_id: SessionId::new(parent_session_id),
            agent_role: agent_role.parse()?,
            prompt,
        });
    }

    if args.next().is_some() {
        bail!("prompt mode accepts exactly one prompt argument");
    }

    Ok(CliCommand::Run { prompt: first })
}
