use crate::config::ThinkingMode;
use crate::state::fork_history::ForkTurns;

use super::{AgentToolPolicy, AgentType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfile {
    pub id: AgentType,
    pub display_name: String,
    pub description: String,
    pub spawn_guidance: String,
    pub instructions: String,
    pub response_guidance: String,
    pub default_fork_turns: ForkTurns,
    pub default_thinking_mode: Option<ThinkingMode>,
    pub tool_policy: AgentToolPolicy,
}

pub fn all_profiles() -> Vec<AgentProfile> {
    AgentType::ALL
        .into_iter()
        .map(|agent_type| profile_for_type(Some(agent_type)))
        .collect()
}

pub fn profile_for_type(agent_type: Option<AgentType>) -> AgentProfile {
    match agent_type.unwrap_or(AgentType::Worker) {
        AgentType::Explorer => AgentProfile {
            id: AgentType::Explorer,
            display_name: "Explorer".to_string(),
            description: "Read-only codebase exploration.".to_string(),
            spawn_guidance:
                "Use for scoped repository questions, locating relevant files, tracing real code\n\
paths, and gathering grounded context before planning or implementation. Spawn\n\
multiple explorers in parallel when the questions are independent."
                    .to_string(),
            instructions:
                "You are an explorer agent. Your job is to inspect the codebase and return\n\
grounded findings.\n\n\
Work read-only. Prefer search_files for broad discovery and read_file for\n\
targeted files. Trace real code paths before drawing conclusions. Cite exact\n\
files and symbols. Do not edit files, run commands, spawn agents, or propose\n\
implementation unless the parent explicitly asks for next-step options."
                    .to_string(),
            response_guidance:
                "Return relevant paths, verified findings, inferred conclusions, open questions,\n\
and the recommended next step."
                    .to_string(),
            default_fork_turns: ForkTurns::None,
            default_thinking_mode: Some(ThinkingMode::Low),
            tool_policy: read_only_policy(),
        },
        AgentType::Planner => AgentProfile {
            id: AgentType::Planner,
            display_name: "Planner".to_string(),
            description: "Create concrete implementation plans without changing product code."
                .to_string(),
            spawn_guidance:
                "Use when the next step is a decision-complete implementation plan rather than\n\
code changes. Ask it for likely files, ordering, dependencies, risks, and\n\
verification steps. Do not use it for direct implementation."
                    .to_string(),
            instructions: "You are a planner agent. Produce a decision-complete implementation plan, not code.\n\n\
Read enough context to make the plan concrete. Identify files likely to change,\n\
data flow, interfaces, risks, and verification steps. Do not edit product code,\n\
run mutating commands, or start implementation. If the task is underspecified,\n\
state assumptions and choose the smallest safe scope."
                .to_string(),
            response_guidance:
                "Return objective, assumptions, file map, ordered implementation steps,\n\
verification commands, risks, and handoff notes. The plan should be executable\n\
by a worker without requiring new design decisions."
                    .to_string(),
            default_fork_turns: ForkTurns::None,
            default_thinking_mode: Some(ThinkingMode::Medium),
            tool_policy: read_only_policy(),
        },
        AgentType::Reviewer => AgentProfile {
            id: AgentType::Reviewer,
            display_name: "Reviewer".to_string(),
            description: "Review work and provide a gate without implementing fixes.".to_string(),
            spawn_guidance:
                "Use as a review gate after implementation or before a final handoff. Ask it to\n\
check correctness, regressions, security, missing tests, and maintainability.\n\
Do not ask it to fix issues."
                    .to_string(),
            instructions: "You are a reviewer agent. Review only; do not implement.\n\n\
Prioritize correctness, security, behavior regressions, missing tests, and\n\
maintainability risks. Lead with concrete findings. Avoid style-only comments\n\
unless they hide a real behavior or maintenance risk."
                .to_string(),
            response_guidance:
                "Lead with findings ordered by severity. Each finding should include severity,\n\
file reference, reason, and suggested fix direction. End with gate: pass,\n\
pass_with_concerns, or fail."
                    .to_string(),
            default_fork_turns: ForkTurns::All,
            default_thinking_mode: Some(ThinkingMode::High),
            tool_policy: read_only_policy(),
        },
        AgentType::Worker => AgentProfile {
            id: AgentType::Worker,
            display_name: "Worker".to_string(),
            description: "Execute scoped implementation or debugging tasks.".to_string(),
            spawn_guidance:
                "Use for scoped implementation, debugging, test fixing, or mechanical code work.\n\
Assign clear file/module ownership and verification expectations. Remind the\n\
worker that other agents or the user may be editing the same workspace."
                    .to_string(),
            instructions:
                "You are a worker agent. Execute the scoped task given by the parent.\n\n\
Keep changes narrow, follow local patterns, and verify the behavior you\n\
changed. Do not broaden scope without reporting the reason."
                    .to_string(),
            response_guidance:
                "Return changed files, verification run, result, and remaining caveats.".to_string(),
            default_fork_turns: ForkTurns::None,
            default_thinking_mode: None,
            tool_policy: AgentToolPolicy::all(),
        },
    }
}

pub fn render_spawn_agent_type_description() -> String {
    let mut lines = vec![
        "Optional built-in agent profile for the child. Defaults to `worker`.".to_string(),
        String::new(),
        "Available profiles:".to_string(),
    ];
    for profile in all_profiles() {
        lines.push(render_profile_for_spawn_schema(&profile));
    }
    lines.join("\n")
}

fn render_profile_for_spawn_schema(profile: &AgentProfile) -> String {
    format!(
        "{name} ({display_name}):\n\
  What it is: {description}\n\
  When to spawn: {spawn_guidance}\n\
  Visible tools: {tools}\n\
  Defaults: fork_turns={fork_turns}, thinking_mode={thinking_mode}",
        name = profile.id.as_str(),
        display_name = profile.display_name,
        description = profile.description,
        spawn_guidance = profile.spawn_guidance,
        tools = tool_policy_summary(&profile.tool_policy),
        fork_turns = profile.default_fork_turns.label(),
        thinking_mode = profile
            .default_thinking_mode
            .map(ThinkingMode::label)
            .unwrap_or("inherited"),
    )
}

fn tool_policy_summary(policy: &AgentToolPolicy) -> String {
    match policy {
        AgentToolPolicy::All => "all tools allowed by the session policy".to_string(),
        AgentToolPolicy::AllowOnly(names) => names.join(", "),
    }
}

fn read_only_policy() -> AgentToolPolicy {
    AgentToolPolicy::allow_only(["read_file", "search_files", "list_agents"])
}
