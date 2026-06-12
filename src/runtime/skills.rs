use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillConfig {
    pub enabled: bool,
    pub max_metadata_chars: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillScope {
    Repo,
    User,
}

impl SkillScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Repo => "repo",
            Self::User => "user",
        }
    }

    /// Lower rank wins when the metadata budget forces omissions. Repo skills
    /// are closer to the task than user-wide skills, so they are kept first.
    fn prompt_rank(self) -> u8 {
        match self {
            Self::Repo => 0,
            Self::User => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub scope: SkillScope,
    /// When false, the skill is hidden from the model-visible available-skills
    /// list and can only be loaded through an explicit `$name` invocation.
    pub allow_implicit_invocation: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillWarningKind {
    DuplicateName,
    InvalidMetadata,
    ReadError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillWarning {
    pub kind: SkillWarningKind,
    pub scope: SkillScope,
    pub name: String,
    pub paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkillCatalog {
    pub skills: Vec<SkillMetadata>,
    pub warnings: Vec<SkillWarning>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderedSkills {
    pub text: String,
    /// Number of skills dropped entirely because the budget could not fit even
    /// their name and path.
    pub omitted: usize,
    /// True when at least one description was shortened to fit the budget.
    pub descriptions_shortened: bool,
    /// True when anything was dropped or shortened (omission or truncation).
    pub truncated: bool,
}

#[derive(Debug)]
pub enum SkillError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    InvalidFrontmatter {
        path: PathBuf,
        message: String,
    },
}

impl std::fmt::Display for SkillError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "failed to read {}: {}", path.display(), source)
            }
            Self::InvalidFrontmatter { path, message } => {
                write!(
                    formatter,
                    "invalid frontmatter in {}: {}",
                    path.display(),
                    message
                )
            }
        }
    }
}

impl std::error::Error for SkillError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidFrontmatter { .. } => None,
        }
    }
}

impl Default for SkillConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_metadata_chars: 8_000,
        }
    }
}

pub fn load_skills(
    workspace_root: &Path,
    user_roots: &[PathBuf],
    config: &SkillConfig,
) -> SkillCatalog {
    if !config.enabled {
        return SkillCatalog::default();
    }

    let repo_root = workspace_root.join(".agents").join("skills");
    let (repo_skills, mut warnings) = load_scope_skills(&repo_root, SkillScope::Repo);

    let repo_paths_by_name = repo_skills
        .iter()
        .map(|skill| (skill.name.clone(), skill.path.clone()))
        .collect::<HashMap<_, _>>();

    let mut skills = repo_skills;
    let mut user_paths_by_name: HashMap<String, PathBuf> = HashMap::new();
    for root in user_roots {
        let (user_skills, user_warnings) = load_scope_skills(root, SkillScope::User);
        warnings.extend(user_warnings);

        for skill in user_skills {
            if let Some(repo_path) = repo_paths_by_name.get(&skill.name) {
                // The repo skill takes priority; surface the shadowed user
                // skill so the collision is visible instead of silently
                // dropping configured global/user skills.
                push_duplicate_warning(
                    &mut warnings,
                    SkillScope::User,
                    skill.name,
                    vec![repo_path.clone(), skill.path],
                );
                continue;
            }
            if let Some(existing_path) = user_paths_by_name.get(&skill.name) {
                push_duplicate_warning(
                    &mut warnings,
                    SkillScope::User,
                    skill.name,
                    vec![existing_path.clone(), skill.path],
                );
                continue;
            }

            user_paths_by_name.insert(skill.name.clone(), skill.path.clone());
            skills.push(skill);
        }
    }

    SkillCatalog { skills, warnings }
}

pub fn render_available_skills(catalog: &SkillCatalog, max_chars: usize) -> RenderedSkills {
    // Only implicit-allowed skills are exposed to the model. Explicit-only
    // skills stay invocable through `$name` but are never advertised here.
    let mut lines = catalog
        .skills
        .iter()
        .filter(|skill| skill.allow_implicit_invocation)
        .map(SkillLine::new)
        .collect::<Vec<_>>();
    lines.sort_by(|a, b| {
        a.scope_rank
            .cmp(&b.scope_rank)
            .then_with(|| a.name.cmp(&b.name))
    });

    if lines.is_empty() && catalog.warnings.is_empty() {
        return RenderedSkills::default();
    }

    let (mut text, mut report) = render_skill_lines(&lines, max_chars);
    let mut remaining = max_chars.saturating_sub(text.chars().count());

    if !catalog.warnings.is_empty() {
        if !text.is_empty() {
            push_render_entry(&mut text, "\n", &mut remaining);
        }
        if !push_render_entry(&mut text, "Skill warnings:\n", &mut remaining) {
            report.truncated = true;
        }
        for warning in &catalog.warnings {
            let name = if warning.name.is_empty() {
                String::new()
            } else {
                format!(" {}", warning.name)
            };
            let entry = format!(
                "- {} [{}]{}: {}\n",
                skill_warning_kind_label(&warning.kind),
                warning.scope.as_str(),
                name,
                warning
                    .paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if !push_render_entry(&mut text, &entry, &mut remaining) {
                report.truncated = true;
                break;
            }
        }
    }

    RenderedSkills {
        text,
        omitted: report.omitted,
        descriptions_shortened: report.descriptions_shortened,
        truncated: report.truncated,
    }
}

#[derive(Default)]
struct SkillRenderReport {
    omitted: usize,
    descriptions_shortened: bool,
    truncated: bool,
}

struct SkillLine {
    name: String,
    scope: &'static str,
    scope_rank: u8,
    description: Vec<char>,
    path: String,
}

impl SkillLine {
    fn new(skill: &SkillMetadata) -> Self {
        Self {
            name: skill.name.clone(),
            scope: skill.scope.as_str(),
            scope_rank: skill.scope.prompt_rank(),
            description: skill.description.chars().collect(),
            path: skill.path.display().to_string(),
        }
    }

    /// Cost (in chars, including the trailing newline) of the line with no
    /// description text at all.
    fn minimum_cost(&self) -> usize {
        self.render_with_description_chars(0).chars().count() + 1
    }

    fn full_cost(&self) -> usize {
        self.render_with_description_chars(self.description.len())
            .chars()
            .count()
            + 1
    }

    fn render_with_description_chars(&self, description_chars: usize) -> String {
        if description_chars == 0 {
            format!("- ${} [{}]: (file: {})", self.name, self.scope, self.path)
        } else {
            let description = self.description[..description_chars]
                .iter()
                .collect::<String>();
            format!(
                "- ${} [{}]: {} (file: {})",
                self.name, self.scope, description, self.path
            )
        }
    }
}

/// Render the skill list within `max_chars`. Descriptions are shortened evenly
/// before any whole skill is dropped, mirroring the Codex skills budget so the
/// model keeps seeing every skill's name and path for as long as possible.
fn render_skill_lines(lines: &[SkillLine], max_chars: usize) -> (String, SkillRenderReport) {
    let mut report = SkillRenderReport::default();
    if lines.is_empty() {
        return (String::new(), report);
    }

    let full_cost: usize = lines.iter().map(SkillLine::full_cost).sum();
    if full_cost <= max_chars {
        let text = lines
            .iter()
            .map(|line| line.render_with_description_chars(line.description.len()))
            .collect::<Vec<_>>()
            .join("\n");
        return (format!("{text}\n"), report);
    }

    let minimum_cost: usize = lines.iter().map(SkillLine::minimum_cost).sum();
    if minimum_cost <= max_chars {
        let allocations = distribute_description_budget(lines, max_chars - minimum_cost);
        let mut rendered = Vec::with_capacity(lines.len());
        for (line, allocated) in lines.iter().zip(&allocations) {
            if *allocated < line.description.len() {
                report.descriptions_shortened = true;
                report.truncated = true;
            }
            rendered.push(line.render_with_description_chars(*allocated));
        }
        return (format!("{}\n", rendered.join("\n")), report);
    }

    // Even the minimum lines do not all fit: keep as many name+path lines as
    // possible in priority order and omit the rest.
    let mut rendered = Vec::new();
    let mut used = 0usize;
    for line in lines {
        let cost = line.minimum_cost();
        if used + cost <= max_chars {
            used += cost;
            rendered.push(line.render_with_description_chars(0));
            report.descriptions_shortened = true;
        } else {
            report.omitted = report.omitted.saturating_add(1);
        }
    }
    report.truncated = true;
    let text = if rendered.is_empty() {
        String::new()
    } else {
        format!("{}\n", rendered.join("\n"))
    };
    (text, report)
}

/// Hand out description characters one at a time across all skills so short
/// descriptions never strand budget that a longer description could use.
fn distribute_description_budget(lines: &[SkillLine], extra_budget: usize) -> Vec<usize> {
    let mut allocations = vec![0usize; lines.len()];
    let mut remaining = extra_budget;

    loop {
        let mut changed = false;
        for (index, line) in lines.iter().enumerate() {
            if allocations[index] >= line.description.len() {
                continue;
            }
            // First description char also pays for the separating space; every
            // later char costs one. Model the first char as costing 2.
            let delta = if allocations[index] == 0 { 2 } else { 1 };
            if delta <= remaining {
                allocations[index] += 1;
                remaining -= delta;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    allocations
}

fn push_render_entry(text: &mut String, entry: &str, remaining: &mut usize) -> bool {
    // All-or-nothing: never emit a partially written entry. A half-written
    // metadata line is not useful and would make the `omitted` count include a
    // skill that is also partially shown.
    let entry_chars = entry.chars().count();
    if entry_chars <= *remaining {
        text.push_str(entry);
        *remaining -= entry_chars;
        return true;
    }

    false
}

fn skill_warning_kind_label(kind: &SkillWarningKind) -> &'static str {
    match kind {
        SkillWarningKind::DuplicateName => "duplicate_name",
        SkillWarningKind::InvalidMetadata => "invalid_metadata",
        SkillWarningKind::ReadError => "read_error",
    }
}

pub fn resolve_explicit_skill_mentions(prompt: &str, catalog: &SkillCatalog) -> Vec<SkillMetadata> {
    let by_name = catalog
        .skills
        .iter()
        .map(|skill| (skill.name.as_str(), skill))
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    let mut resolved = Vec::new();
    let chars = prompt.chars().collect::<Vec<_>>();
    let mut index = 0;

    while index < chars.len() {
        if chars[index] != '$' {
            index += 1;
            continue;
        }

        // Require a word boundary before `$` so mentions embedded in a larger
        // token (e.g. `foo$review`, a shell variable, or inline code) do not
        // accidentally load a skill body.
        if index > 0 && is_skill_name_char(chars[index - 1]) {
            index += 1;
            continue;
        }

        let start = index + 1;
        let mut end = start;
        while end < chars.len() && is_skill_name_char(chars[end]) {
            end += 1;
        }

        if start == end {
            index += 1;
            continue;
        }

        let name = chars[start..end].iter().collect::<String>();
        if let Some(skill) = by_name.get(name.as_str()) {
            if seen.insert(name) {
                resolved.push((*skill).clone());
            }
        }
        index = end;
    }

    resolved
}

pub fn load_skill_body(metadata: &SkillMetadata) -> Result<String, SkillError> {
    fs::read_to_string(&metadata.path).map_err(|source| SkillError::Io {
        path: metadata.path.clone(),
        source,
    })
}

fn load_scope_skills(root: &Path, scope: SkillScope) -> (Vec<SkillMetadata>, Vec<SkillWarning>) {
    let mut warnings = Vec::new();
    let mut by_name = BTreeMap::new();

    if !root.is_dir() {
        return (Vec::new(), warnings);
    }

    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => {
            warnings.push(SkillWarning {
                kind: SkillWarningKind::ReadError,
                scope,
                name: String::new(),
                paths: vec![root.to_path_buf()],
            });
            return (Vec::new(), warnings);
        }
    };

    let mut skill_dirs = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    skill_dirs.sort();

    for skill_dir in skill_dirs {
        let skill_path = skill_dir.join("SKILL.md");
        if !skill_path.is_file() {
            continue;
        }

        match read_skill_metadata(&skill_path, scope) {
            Ok(skill) => insert_skill(skill, &mut by_name, &mut warnings),
            Err(kind) => warnings.push(SkillWarning {
                kind,
                scope,
                name: String::new(),
                paths: vec![skill_path],
            }),
        }
    }

    (by_name.into_values().collect(), warnings)
}

fn insert_skill(
    skill: SkillMetadata,
    by_name: &mut BTreeMap<String, SkillMetadata>,
    warnings: &mut Vec<SkillWarning>,
) {
    if let Some(existing) = by_name.get(&skill.name) {
        push_duplicate_warning(
            warnings,
            skill.scope,
            skill.name,
            vec![existing.path.clone(), skill.path],
        );
        return;
    }

    by_name.insert(skill.name.clone(), skill);
}

fn push_duplicate_warning(
    warnings: &mut Vec<SkillWarning>,
    scope: SkillScope,
    name: String,
    paths: Vec<PathBuf>,
) {
    if let Some(warning) = warnings.iter_mut().find(|warning| {
        warning.kind == SkillWarningKind::DuplicateName
            && warning.scope == scope
            && warning.name == name
    }) {
        for path in paths {
            if !warning.paths.contains(&path) {
                warning.paths.push(path);
            }
        }
        return;
    }

    warnings.push(SkillWarning {
        kind: SkillWarningKind::DuplicateName,
        scope,
        name,
        paths,
    });
}

fn read_skill_metadata(path: &Path, scope: SkillScope) -> Result<SkillMetadata, SkillWarningKind> {
    let frontmatter = read_frontmatter(path).map_err(|error| match error {
        SkillError::Io { .. } => SkillWarningKind::ReadError,
        SkillError::InvalidFrontmatter { .. } => SkillWarningKind::InvalidMetadata,
    })?;
    let parsed = parse_frontmatter_fields(&frontmatter);
    let name = parsed.name.unwrap_or_default().trim().to_string();
    let description = parsed.description.unwrap_or_default().trim().to_string();
    let allow_implicit_invocation = match parsed.allow_implicit_invocation {
        ParsedBool::Missing => true,
        ParsedBool::Valid(value) => value,
        ParsedBool::Invalid => return Err(SkillWarningKind::InvalidMetadata),
    };

    if !is_valid_skill_name(&name)
        || name.chars().count() > 64
        || description.is_empty()
        || description.chars().count() > 1024
    {
        return Err(SkillWarningKind::InvalidMetadata);
    }

    Ok(SkillMetadata {
        name,
        description,
        path: path.to_path_buf(),
        scope,
        allow_implicit_invocation,
    })
}

fn read_frontmatter(path: &Path) -> Result<String, SkillError> {
    let file = File::open(path).map_err(|source| SkillError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();

    if reader
        .read_until(b'\n', &mut line)
        .map_err(|source| SkillError::Io {
            path: path.to_path_buf(),
            source,
        })?
        == 0
    {
        return Err(invalid_frontmatter(path, "missing opening delimiter"));
    }

    let first_line = decode_frontmatter_line(path, &line)?;
    if first_line
        .trim_start_matches('\u{feff}')
        .trim_end_matches(['\r', '\n'])
        .trim()
        != "---"
    {
        return Err(invalid_frontmatter(path, "missing opening delimiter"));
    }

    let mut frontmatter = String::new();
    loop {
        line.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut line)
            .map_err(|source| SkillError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        if bytes_read == 0 {
            return Err(invalid_frontmatter(path, "missing closing delimiter"));
        }

        let decoded = decode_frontmatter_line(path, &line)?;
        if decoded.trim_end_matches(['\r', '\n']).trim() == "---" {
            return Ok(frontmatter);
        }
        frontmatter.push_str(decoded);
    }
}

fn decode_frontmatter_line<'a>(path: &Path, line: &'a [u8]) -> Result<&'a str, SkillError> {
    std::str::from_utf8(line).map_err(|_| invalid_frontmatter(path, "frontmatter is not utf-8"))
}

fn invalid_frontmatter(path: &Path, message: &str) -> SkillError {
    SkillError::InvalidFrontmatter {
        path: path.to_path_buf(),
        message: message.to_string(),
    }
}

#[derive(Default)]
struct ParsedFrontmatter {
    name: Option<String>,
    description: Option<String>,
    allow_implicit_invocation: ParsedBool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ParsedBool {
    #[default]
    Missing,
    Valid(bool),
    Invalid,
}

fn parse_frontmatter_fields(frontmatter: &str) -> ParsedFrontmatter {
    let mut parsed = ParsedFrontmatter::default();

    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let value = parse_yaml_scalar(value);

        match key.trim() {
            "name" => parsed.name = Some(value),
            "description" => parsed.description = Some(value),
            "allow_implicit_invocation" => {
                parsed.allow_implicit_invocation = parse_yaml_bool(&value)
                    .map(ParsedBool::Valid)
                    .unwrap_or(ParsedBool::Invalid);
            }
            _ => {}
        }
    }

    parsed
}

fn parse_yaml_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn parse_yaml_scalar(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        return unescape_double_quoted_yaml_scalar(&trimmed[1..trimmed.len() - 1]);
    }
    if trimmed.len() >= 2 && trimmed.starts_with('\'') && trimmed.ends_with('\'') {
        return trimmed[1..trimmed.len() - 1].replace("''", "'");
    }

    trimmed.to_string()
}

fn unescape_double_quoted_yaml_scalar(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars();

    while let Some(char) = chars.next() {
        if char != '\\' {
            output.push(char);
            continue;
        }

        match chars.next() {
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some('t') => output.push('\t'),
            Some('"') => output.push('"'),
            Some('\\') => output.push('\\'),
            Some(other) => output.push(other),
            None => output.push('\\'),
        }
    }

    output
}

fn is_skill_name_char(char: char) -> bool {
    char.is_ascii_alphanumeric() || matches!(char, '-' | '_')
}

fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(is_skill_name_char)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentConfig;
    use crate::runtime::agent_profile::AgentToolPolicy;
    use crate::runtime::exec_session::ExecSessionManager;
    use crate::runtime::policy::PolicyManager;
    use crate::tools::read_file::ReadFileTool;
    use crate::tools::registry::{ToolContext, ToolRegistry};
    use crate::types::{ToolCall, ToolStatus};
    use serde_json::json;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let unique = format!(
                "exagent-skills-test-{}-{}-{}-{}",
                label,
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            let path = std::env::temp_dir().join(unique);
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_skill(
        root: &Path,
        dir_name: &str,
        name: &str,
        description: &str,
        body: &str,
    ) -> PathBuf {
        let skill_dir = root.join(dir_name);
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n"),
        )
        .unwrap();
        path
    }

    fn write_skill_bytes(root: &Path, dir_name: &str, bytes: &[u8]) -> PathBuf {
        let skill_dir = root.join(dir_name);
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(&path, bytes).unwrap();
        path
    }

    fn catalog_with_repo_and_user(workspace: &Path, user_root: &Path) -> SkillCatalog {
        load_skills(
            workspace,
            &[user_root.to_path_buf()],
            &SkillConfig::default(),
        )
    }

    fn skill_meta(name: &str, description: &str, path: &str, scope: SkillScope) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            description: description.to_string(),
            path: PathBuf::from(path),
            scope,
            allow_implicit_invocation: true,
        }
    }

    fn write_skill_with_frontmatter(root: &Path, dir_name: &str, frontmatter: &str) -> PathBuf {
        let skill_dir = root.join(dir_name);
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(&path, format!("---\n{frontmatter}\n---\n\nbody\n")).unwrap();
        path
    }

    #[tokio::test]
    async fn catalog_user_skill_path_is_readable_by_read_file_tool() {
        let temp = TempDir::new("catalog-read-file-contract");
        let user_root = temp.path.join("user-skills");
        write_skill(
            &user_root,
            "alpha",
            "alpha",
            "Alpha skill description",
            "Full body should be readable through read_file.",
        );
        let config = AgentConfig {
            workspace_root: temp.path.join("workspace"),
            cwd: temp.path.join("workspace"),
            skills_user_roots: vec![user_root],
            ..AgentConfig::default()
        };
        fs::create_dir_all(&config.workspace_root).unwrap();

        let catalog = load_skills(
            &config.workspace_root,
            &config.skills_user_roots,
            &SkillConfig::default(),
        );
        let skill_path = catalog
            .skills
            .iter()
            .find(|skill| skill.name == "alpha")
            .expect("catalog contains user skill")
            .path
            .clone();

        let mut registry = ToolRegistry::new();
        registry.register(ReadFileTool);
        let ctx = ToolContext {
            config,
            thread_id: None,
            turn_id: None,
            tool_invocation_id: None,
            exec_sessions: Arc::new(ExecSessionManager::default()),
            exec_output_sink: None,
            policy: Arc::new(PolicyManager::default()),
            agent_tool_policy: AgentToolPolicy::all(),
            inbox: None,
            goal_api: None,
        };

        let result = registry
            .execute(
                ToolCall {
                    id: "call_catalog_skill_read".into(),
                    name: "read_file".into(),
                    arguments: json!({"path": skill_path.display().to_string()}),
                    thought_signature: None,
                },
                Some(&ctx),
            )
            .await;

        assert_eq!(result.status, ToolStatus::Success);
        assert!(result
            .content
            .contains("Full body should be readable through read_file."));
    }

    #[test]
    fn parses_valid_frontmatter_metadata() {
        let temp = TempDir::new("metadata");
        let repo_root = temp.path.join(".agents").join("skills");
        let skill_path = write_skill(
            &repo_root,
            "alpha",
            "alpha",
            "Alpha skill description",
            "Full body should not be metadata.",
        );

        let catalog = load_skills(&temp.path, &[], &SkillConfig::default());

        assert_eq!(catalog.warnings, Vec::new());
        assert_eq!(
            catalog.skills,
            vec![SkillMetadata {
                name: "alpha".to_string(),
                description: "Alpha skill description".to_string(),
                path: skill_path,
                scope: SkillScope::Repo,
                allow_implicit_invocation: true,
            }]
        );
    }

    #[test]
    fn prefers_repo_skill_over_user_skill_with_same_name() {
        let temp = TempDir::new("repo-priority");
        let repo_root = temp.path.join(".agents").join("skills");
        let user_root = temp.path.join("user-skills");
        let repo_path = write_skill(
            &repo_root,
            "shared",
            "shared",
            "Repo description wins",
            "repo body",
        );
        let user_path = write_skill(
            &user_root,
            "shared",
            "shared",
            "User description loses",
            "user body",
        );
        write_skill(
            &user_root,
            "user-only",
            "user-only",
            "User only",
            "user body",
        );

        let catalog = catalog_with_repo_and_user(&temp.path, &user_root);

        assert_eq!(catalog.skills.len(), 2);
        assert_eq!(
            catalog
                .skills
                .iter()
                .find(|skill| skill.name == "shared")
                .unwrap(),
            &SkillMetadata {
                name: "shared".to_string(),
                description: "Repo description wins".to_string(),
                path: repo_path.clone(),
                scope: SkillScope::Repo,
                allow_implicit_invocation: true,
            }
        );
        assert!(catalog.skills.iter().any(|skill| skill.name == "user-only"));
        // The shadowed user skill is surfaced as a duplicate-name warning.
        assert_eq!(
            catalog.warnings,
            vec![SkillWarning {
                kind: SkillWarningKind::DuplicateName,
                scope: SkillScope::User,
                name: "shared".to_string(),
                paths: vec![repo_path, user_path],
            }]
        );
    }

    #[test]
    fn loads_multiple_user_skill_roots_and_preserves_repo_priority() {
        let temp = TempDir::new("multi-user-roots");
        let repo_root = temp.path.join(".agents").join("skills");
        let first_user_root = temp.path.join("first-user-skills");
        let second_user_root = temp.path.join("second-user-skills");

        let repo_shared = write_skill(
            &repo_root,
            "shared",
            "shared",
            "Repo shared description",
            "repo body",
        );
        let first_unique = write_skill(
            &first_user_root,
            "first-only",
            "first-only",
            "First user root skill",
            "first body",
        );
        let shadowed_user = write_skill(
            &first_user_root,
            "shared",
            "shared",
            "Shadowed user skill",
            "shadowed body",
        );
        let second_unique = write_skill(
            &second_user_root,
            "second-only",
            "second-only",
            "Second user root skill",
            "second body",
        );

        let roots = vec![first_user_root, second_user_root];
        let catalog = load_skills(&temp.path, &roots, &SkillConfig::default());

        assert_eq!(catalog.skills.len(), 3);
        assert!(catalog
            .skills
            .iter()
            .any(|skill| skill.name == "first-only" && skill.path == first_unique));
        assert!(catalog
            .skills
            .iter()
            .any(|skill| skill.name == "second-only" && skill.path == second_unique));
        assert_eq!(
            catalog
                .skills
                .iter()
                .find(|skill| skill.name == "shared")
                .unwrap()
                .path,
            repo_shared
        );
        assert_eq!(
            catalog.warnings,
            vec![SkillWarning {
                kind: SkillWarningKind::DuplicateName,
                scope: SkillScope::User,
                name: "shared".to_string(),
                paths: vec![repo_shared, shadowed_user],
            }]
        );
    }

    #[test]
    fn records_warning_for_duplicate_name_within_same_scope() {
        let temp = TempDir::new("duplicate");
        let repo_root = temp.path.join(".agents").join("skills");
        let first = write_skill(&repo_root, "first", "duplicate", "First", "first body");
        let second = write_skill(&repo_root, "second", "duplicate", "Second", "second body");

        let catalog = load_skills(&temp.path, &[], &SkillConfig::default());

        assert_eq!(
            catalog
                .skills
                .iter()
                .filter(|skill| skill.name == "duplicate")
                .count(),
            1
        );
        assert_eq!(
            catalog.warnings,
            vec![SkillWarning {
                kind: SkillWarningKind::DuplicateName,
                scope: SkillScope::Repo,
                name: "duplicate".to_string(),
                paths: vec![first, second],
            }]
        );
        let rendered = render_available_skills(&catalog, 2048);
        assert!(rendered.text.contains("Skill warnings"));
        assert!(rendered.text.contains("duplicate_name [repo] duplicate"));
    }

    #[test]
    fn rejects_skill_names_that_cannot_be_dollar_invoked() {
        let temp = TempDir::new("invalid-name");
        let repo_root = temp.path.join(".agents").join("skills");
        let invalid = write_skill(
            &repo_root,
            "invalid",
            "foo.bar",
            "Invalid name",
            "invalid body",
        );

        let catalog = load_skills(&temp.path, &[], &SkillConfig::default());
        let rendered = render_available_skills(&catalog, 2048);

        assert!(catalog.skills.is_empty());
        assert_eq!(
            catalog.warnings,
            vec![SkillWarning {
                kind: SkillWarningKind::InvalidMetadata,
                scope: SkillScope::Repo,
                name: String::new(),
                paths: vec![invalid],
            }]
        );
        assert!(!rendered.text.contains("$foo.bar"));
        assert!(rendered.text.contains("invalid_metadata [repo]:"));
    }

    #[test]
    fn resolves_dollar_prefixed_explicit_skill_mentions() {
        let catalog = SkillCatalog {
            skills: vec![
                skill_meta(
                    "alpha-tool",
                    "Alpha",
                    "/tmp/alpha/SKILL.md",
                    SkillScope::Repo,
                ),
                skill_meta("beta", "Beta", "/tmp/beta/SKILL.md", SkillScope::User),
            ],
            warnings: Vec::new(),
        };

        let resolved = resolve_explicit_skill_mentions(
            "Please use $alpha-tool, ignore $missing, then $beta.",
            &catalog,
        );

        assert_eq!(
            resolved
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha-tool", "beta"]
        );
    }

    #[test]
    fn ignores_dollar_mentions_without_a_word_boundary() {
        let catalog = SkillCatalog {
            skills: vec![skill_meta(
                "review",
                "Review",
                "/tmp/review/SKILL.md",
                SkillScope::Repo,
            )],
            warnings: Vec::new(),
        };

        // Embedded in a larger token: must not resolve.
        assert!(resolve_explicit_skill_mentions("foo$review", &catalog).is_empty());
        assert!(resolve_explicit_skill_mentions("PATH=x$review", &catalog).is_empty());

        // A real boundary (start, whitespace, punctuation) still resolves.
        let resolved = resolve_explicit_skill_mentions("run $review now ($review)", &catalog);
        assert_eq!(
            resolved
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["review"]
        );
    }

    #[test]
    fn untriggered_metadata_scan_does_not_read_full_body() {
        let temp = TempDir::new("no-body-load");
        let repo_root = temp.path.join(".agents").join("skills");
        write_skill_bytes(
            &repo_root,
            "binary-body",
            b"---\nname: binary-body\ndescription: Metadata is utf8\n---\n\n\xff\xfe\xfd",
        );

        let catalog = load_skills(&temp.path, &[], &SkillConfig::default());
        let rendered = render_available_skills(&catalog, 2048);
        let resolved = resolve_explicit_skill_mentions("No explicit mention here.", &catalog);

        assert_eq!(catalog.skills.len(), 1);
        assert!(catalog.warnings.is_empty());
        assert!(!rendered.text.contains("\u{fffd}"));
        assert!(resolved.is_empty());
    }

    #[test]
    fn render_includes_name_scope_description_and_path() {
        let catalog = SkillCatalog {
            skills: vec![skill_meta("alpha", "Alpha desc", "/a", SkillScope::Repo)],
            warnings: Vec::new(),
        };

        let rendered = render_available_skills(&catalog, 4096);

        assert!(!rendered.truncated);
        assert!(!rendered.descriptions_shortened);
        assert_eq!(rendered.omitted, 0);
        assert!(rendered
            .text
            .contains("- $alpha [repo]: Alpha desc (file: /a)"));
    }

    #[test]
    fn render_truncates_descriptions_before_omitting_skills() {
        let catalog = SkillCatalog {
            skills: vec![
                skill_meta("alpha", "AAAAAAAA", "/a", SkillScope::Repo),
                skill_meta("beta", "BBBBBBBB", "/b", SkillScope::Repo),
            ],
            warnings: Vec::new(),
        };
        let full = render_available_skills(&catalog, 4096).text.chars().count();

        // Between the minimum (name+path only) and full cost: descriptions get
        // shortened but no skill is dropped.
        let rendered = render_available_skills(&catalog, full - 4);

        assert_eq!(rendered.omitted, 0);
        assert!(rendered.descriptions_shortened);
        assert!(rendered.truncated);
        assert!(rendered.text.contains("$alpha"));
        assert!(rendered.text.contains("$beta"));
        assert!(rendered.text.contains("/a"));
        assert!(rendered.text.contains("/b"));
    }

    #[test]
    fn render_omits_lowest_priority_skills_when_minimum_lines_exceed_budget() {
        let catalog = SkillCatalog {
            skills: vec![
                skill_meta("alpha", "desc", "/a", SkillScope::Repo),
                skill_meta("zulu", "desc", "/z", SkillScope::User),
            ],
            warnings: Vec::new(),
        };

        // Room for roughly one minimum line only.
        let rendered = render_available_skills(&catalog, 30);

        assert!(rendered.truncated);
        assert!(rendered.omitted >= 1);
        // Repo scope outranks user scope, so alpha is kept and zulu is dropped.
        assert!(rendered.text.contains("$alpha"));
        assert!(!rendered.text.contains("$zulu"));
    }

    #[test]
    fn explicit_only_skill_is_hidden_from_list_but_still_resolvable() {
        let catalog = SkillCatalog {
            skills: vec![
                skill_meta("visible", "Implicit skill", "/v", SkillScope::Repo),
                SkillMetadata {
                    allow_implicit_invocation: false,
                    ..skill_meta("secret", "Explicit only", "/s", SkillScope::Repo)
                },
            ],
            warnings: Vec::new(),
        };

        let rendered = render_available_skills(&catalog, 4096);
        assert!(rendered.text.contains("$visible"));
        assert!(!rendered.text.contains("$secret"));

        // It can still be invoked explicitly.
        let resolved = resolve_explicit_skill_mentions("use $secret please", &catalog);
        assert_eq!(
            resolved.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["secret"]
        );
    }

    #[test]
    fn parses_allow_implicit_invocation_frontmatter() {
        let temp = TempDir::new("implicit-flag");
        let repo_root = temp.path.join(".agents").join("skills");
        write_skill_with_frontmatter(
            &repo_root,
            "explicit-only",
            "name: explicit-only\ndescription: Only explicit\nallow_implicit_invocation: false",
        );
        write_skill_with_frontmatter(
            &repo_root,
            "defaulted",
            "name: defaulted\ndescription: Defaults to implicit",
        );

        let catalog = load_skills(&temp.path, &[], &SkillConfig::default());

        let explicit_only = catalog
            .skills
            .iter()
            .find(|s| s.name == "explicit-only")
            .unwrap();
        let defaulted = catalog
            .skills
            .iter()
            .find(|s| s.name == "defaulted")
            .unwrap();
        assert!(!explicit_only.allow_implicit_invocation);
        assert!(defaulted.allow_implicit_invocation);
    }

    #[test]
    fn rejects_invalid_allow_implicit_invocation_frontmatter() {
        let temp = TempDir::new("invalid-implicit-flag");
        let repo_root = temp.path.join(".agents").join("skills");
        let invalid = write_skill_with_frontmatter(
            &repo_root,
            "invalid",
            "name: invalid\ndescription: Invalid implicit flag\nallow_implicit_invocation: flase",
        );

        let catalog = load_skills(&temp.path, &[], &SkillConfig::default());

        assert!(catalog.skills.is_empty());
        assert_eq!(
            catalog.warnings,
            vec![SkillWarning {
                kind: SkillWarningKind::InvalidMetadata,
                scope: SkillScope::Repo,
                name: String::new(),
                paths: vec![invalid],
            }]
        );
    }

    #[test]
    fn load_skill_body_reads_full_skill_markdown() {
        let temp = TempDir::new("load-body");
        let repo_root = temp.path.join(".agents").join("skills");
        let skill_path = write_skill(
            &repo_root,
            "alpha",
            "alpha",
            "Alpha skill description",
            "Full body should be injectable.",
        );
        let metadata = SkillMetadata {
            name: "alpha".to_string(),
            description: "Alpha skill description".to_string(),
            path: skill_path,
            scope: SkillScope::Repo,
            allow_implicit_invocation: true,
        };

        let body = load_skill_body(&metadata).unwrap();

        assert!(body.contains("name: alpha"));
        assert!(body.contains("Full body should be injectable."));
    }

    #[test]
    fn disabled_config_returns_empty_catalog() {
        let temp = TempDir::new("disabled");
        let repo_root = temp.path.join(".agents").join("skills");
        write_skill(&repo_root, "alpha", "alpha", "Alpha", "body");

        let catalog = load_skills(
            &temp.path,
            &[],
            &SkillConfig {
                enabled: false,
                max_metadata_chars: 1024,
            },
        );

        assert!(catalog.skills.is_empty());
        assert!(catalog.warnings.is_empty());
    }
}
