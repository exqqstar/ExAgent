use crate::config::AgentConfig;
use crate::runtime::context::ContextManager;
use crate::runtime::project_docs::{load_project_docs, ProjectDocConfig};
use crate::runtime::skills::{
    load_skill_body, load_skills, render_available_skills, resolve_explicit_skill_mentions,
    SkillConfig,
};
use crate::types::ConversationMessage;

pub(crate) fn refresh_file_backed_contexts(
    config: &AgentConfig,
    workspace_root: &std::path::Path,
    cwd: &std::path::Path,
    prompt: &str,
    context_manager: &mut ContextManager,
) {
    refresh_project_doc_context(config, workspace_root, cwd, context_manager);
    refresh_skill_context(config, workspace_root, prompt, context_manager);
}

fn refresh_project_doc_context(
    config: &AgentConfig,
    workspace_root: &std::path::Path,
    cwd: &std::path::Path,
    context_manager: &mut ContextManager,
) {
    let docs = load_project_docs(
        workspace_root,
        cwd,
        &ProjectDocConfig {
            enabled: config.project_docs_enabled,
            max_bytes: config.project_docs_max_bytes,
            ..ProjectDocConfig::default()
        },
    );
    if let Some(rendered) = docs.render() {
        context_manager.upsert_ephemeral_internal_context(
            "01_project_docs",
            ConversationMessage::injected_user_context("01_project_docs", rendered),
        );
    } else {
        context_manager.clear_ephemeral_internal_context("01_project_docs");
    }
}

const AVAILABLE_SKILLS_INSTRUCTIONS: &str = "## Available skills\n\
A skill is a reusable procedural guide stored in a SKILL.md file. Each entry below lists its name, scope, description, and file path.\n\n\
### How to use skills\n\
- Trigger: if the user names a skill (with `$skill-name` or plain text) OR the task clearly matches a skill's description below, use that skill for this turn. If several match, use the minimal set that covers the request.\n\
- Loading: when you decide to use a skill, open its SKILL.md at the listed path with your file-reading tool and follow it. Listed paths may be outside the workspace but are readable when they are under a configured skill root. An explicitly invoked skill's body may already be included below; if so, use it directly. If reading a listed path fails, say so briefly and continue with the best alternative. Read only what you need.\n\
- Scope: do not carry skills across turns unless they are mentioned again.\n\
- Fallback: if a skill cannot be applied cleanly (missing files, unclear instructions), say so briefly and continue with the best alternative.\n\n\
### Skills\n";

fn refresh_skill_context(
    config: &AgentConfig,
    workspace_root: &std::path::Path,
    prompt: &str,
    context_manager: &mut ContextManager,
) {
    context_manager.clear_ephemeral_internal_context_prefix("03_skill:");
    if !config.skills_enabled {
        context_manager.clear_ephemeral_internal_context("02_available_skills");
        return;
    }

    let skill_config = SkillConfig {
        enabled: config.skills_enabled,
        max_metadata_chars: config.skills_metadata_max_chars,
    };
    let catalog = load_skills(workspace_root, &config.skills_user_roots, &skill_config);
    let rendered = render_available_skills(&catalog, config.skills_metadata_max_chars);
    if rendered.text.trim().is_empty() {
        context_manager.clear_ephemeral_internal_context("02_available_skills");
    } else {
        let mut content = String::from(AVAILABLE_SKILLS_INSTRUCTIONS);
        content.push_str(&rendered.text);
        if rendered.omitted > 0 {
            content.push_str(&format!(
                "\n{} additional skill(s) were omitted to fit the skills context budget.\n",
                rendered.omitted
            ));
        }
        if rendered.descriptions_shortened {
            content.push_str(
                "\nSome skill descriptions were shortened to fit the skills context budget; open the SKILL.md for the full text.\n",
            );
        }
        if rendered.truncated && rendered.omitted == 0 && !rendered.descriptions_shortened {
            content.push_str(
                "\nSome available-skills context was truncated to fit the skills context budget.\n",
            );
        }
        context_manager.upsert_ephemeral_internal_context(
            "02_available_skills",
            ConversationMessage::injected_user_context("02_available_skills", content),
        );
    }

    for skill in resolve_explicit_skill_mentions(prompt, &catalog) {
        let source = format!("03_skill:{}", skill.name);
        let content = match load_skill_body(&skill) {
            Ok(body) => format!(
                "# Skill: {}\n\nSource: {}\n\n{}",
                skill.name,
                skill.path.display(),
                body
            ),
            Err(err) => format!(
                "# Skill: {}\n\nSource: {}\n\nFailed to load skill body: {}",
                skill.name,
                skill.path.display(),
                err
            ),
        };
        context_manager.upsert_ephemeral_internal_context(
            source.clone(),
            ConversationMessage::injected_user_context(source, content),
        );
    }
}
