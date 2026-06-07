use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

pub(crate) const PROJECT_DOC_FILENAME: &str = "AGENTS.md";
pub(crate) const PROJECT_DOC_OVERRIDE_FILENAME: &str = "AGENTS.override.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectDocConfig {
    pub(crate) enabled: bool,
    pub(crate) filenames: Vec<String>,
    pub(crate) max_bytes: usize,
}

impl Default for ProjectDocConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            filenames: vec![
                PROJECT_DOC_FILENAME.to_string(),
                PROJECT_DOC_OVERRIDE_FILENAME.to_string(),
            ],
            max_bytes: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectDocPart {
    pub(crate) path: PathBuf,
    pub(crate) content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectDocWarning {
    pub(crate) path: PathBuf,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProjectDocs {
    pub(crate) parts: Vec<ProjectDocPart>,
    pub(crate) warnings: Vec<ProjectDocWarning>,
    pub(crate) truncated: bool,
    pub(crate) total_bytes: usize,
}

impl ProjectDocs {
    pub(crate) fn render(&self) -> Option<String> {
        if self.parts.is_empty() && self.warnings.is_empty() {
            return None;
        }

        let mut rendered = String::from("# AGENTS.md instructions\n\n");
        if !self.warnings.is_empty() {
            rendered.push_str("## Warnings\n\n");
            for warning in &self.warnings {
                rendered.push_str("- ");
                rendered.push_str(&warning.path.display().to_string());
                rendered.push_str(": ");
                rendered.push_str(&warning.message);
                rendered.push('\n');
            }
            rendered.push('\n');
        }
        for part in &self.parts {
            rendered.push_str("## Source: ");
            rendered.push_str(&part.path.display().to_string());
            rendered.push_str("\n\n");
            rendered.push_str(&part.content);
            if !part.content.ends_with('\n') {
                rendered.push('\n');
            }
            rendered.push('\n');
        }

        if self.truncated {
            rendered
                .push_str("Project instructions were truncated by the configured byte limit.\n");
        }

        Some(rendered)
    }
}

pub(crate) fn load_project_docs(
    workspace_root: &Path,
    cwd: &Path,
    config: &ProjectDocConfig,
) -> ProjectDocs {
    let mut docs = ProjectDocs::default();
    if !config.enabled {
        return docs;
    }

    let dirs = doc_search_dirs(workspace_root, cwd, &mut docs.warnings);
    for dir in dirs {
        for filename in &config.filenames {
            if filename.is_empty() {
                continue;
            }

            let path = dir.join(filename);
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => {
                    docs.warnings.push(ProjectDocWarning {
                        path,
                        message: format!("failed to read project doc: {err}"),
                    });
                    continue;
                }
            };

            let content = match String::from_utf8(bytes) {
                Ok(content) => content,
                Err(_) => {
                    docs.warnings.push(ProjectDocWarning {
                        path,
                        message: "project doc is not valid UTF-8 and was ignored".to_string(),
                    });
                    continue;
                }
            };

            if push_part_with_limit(&mut docs, path, content, config.max_bytes) {
                return docs;
            }
        }
    }

    docs
}

fn push_part_with_limit(
    docs: &mut ProjectDocs,
    path: PathBuf,
    content: String,
    max_bytes: usize,
) -> bool {
    if docs.total_bytes >= max_bytes {
        if !content.is_empty() {
            docs.truncated = true;
            return true;
        }

        docs.parts.push(ProjectDocPart { path, content });
        return false;
    }

    let remaining = max_bytes - docs.total_bytes;
    if content.len() > remaining {
        let content = truncate_at_utf8_boundary(&content, remaining).to_string();
        docs.total_bytes += content.len();
        docs.parts.push(ProjectDocPart { path, content });
        docs.truncated = true;
        return true;
    }

    docs.total_bytes += content.len();
    docs.parts.push(ProjectDocPart { path, content });
    false
}

fn truncate_at_utf8_boundary(content: &str, max_bytes: usize) -> &str {
    if content.len() <= max_bytes {
        return content;
    }

    let mut end = max_bytes;
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    &content[..end]
}

fn doc_search_dirs(
    workspace_root: &Path,
    cwd: &Path,
    warnings: &mut Vec<ProjectDocWarning>,
) -> Vec<PathBuf> {
    let workspace_root = normalize_lexically(workspace_root);
    let cwd = normalize_lexically(cwd);
    let relative_cwd = match cwd.strip_prefix(&workspace_root) {
        Ok(relative_cwd) => relative_cwd,
        Err(_) => {
            warnings.push(ProjectDocWarning {
                path: cwd,
                message: format!(
                    "cwd is not under workspace root {}; project docs were skipped",
                    workspace_root.display()
                ),
            });
            return Vec::new();
        }
    };

    let mut dirs = vec![workspace_root.clone()];
    let mut current = workspace_root;
    for component in relative_cwd.components() {
        if let Component::Normal(part) = component {
            current.push(part);
            dirs.push(current.clone());
        }
    }

    dirs
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                if !matches!(normalized.components().last(), Some(Component::Normal(_)))
                    || !normalized.pop()
                {
                    normalized.push(component.as_os_str());
                }
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "exagent_project_docs_{name}_{}_{}",
                std::process::id(),
                unique
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(path, content).expect("write test file");
    }

    fn default_config(max_bytes: usize) -> ProjectDocConfig {
        ProjectDocConfig {
            enabled: true,
            filenames: vec!["AGENTS.md".to_string(), "AGENTS.override.md".to_string()],
            max_bytes,
        }
    }

    #[test]
    fn loads_parent_before_child_and_main_before_override() {
        let temp = TestDir::new("order");
        let root = temp.path();
        let child = root.join("app");

        write(&root.join("AGENTS.md"), "root main");
        write(&root.join("AGENTS.override.md"), "root override");
        write(&child.join("AGENTS.md"), "child main");
        write(&child.join("AGENTS.override.md"), "child override");

        let docs = load_project_docs(root, &child, &default_config(1024));

        let paths: Vec<_> = docs
            .parts
            .iter()
            .map(|part| {
                part.path
                    .strip_prefix(root)
                    .expect("path under root")
                    .to_path_buf()
            })
            .collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("AGENTS.md"),
                PathBuf::from("AGENTS.override.md"),
                PathBuf::from("app/AGENTS.md"),
                PathBuf::from("app/AGENTS.override.md"),
            ]
        );
        assert_eq!(
            docs.parts
                .iter()
                .map(|part| part.content.as_str())
                .collect::<Vec<_>>(),
            vec!["root main", "root override", "child main", "child override"]
        );
        assert!(!docs.truncated);
        assert_eq!(docs.warnings.len(), 0);
    }

    #[test]
    fn includes_ancestor_docs_between_workspace_root_and_cwd() {
        let temp = TestDir::new("nested");
        let root = temp.path();
        let cwd = root.join("packages/cli/src");

        write(&root.join("AGENTS.md"), "root");
        write(&root.join("packages/AGENTS.md"), "packages");
        write(
            &root.join("packages/cli/AGENTS.override.md"),
            "cli override",
        );

        let docs = load_project_docs(root, &cwd, &default_config(1024));

        assert_eq!(
            docs.parts
                .iter()
                .map(|part| part.content.as_str())
                .collect::<Vec<_>>(),
            vec!["root", "packages", "cli override"]
        );
    }

    #[test]
    fn ignores_missing_files() {
        let temp = TestDir::new("missing");
        let root = temp.path();
        let cwd = root.join("empty");
        fs::create_dir_all(&cwd).expect("create cwd");

        let docs = load_project_docs(root, &cwd, &default_config(1024));

        assert!(docs.parts.is_empty());
        assert!(docs.warnings.is_empty());
        assert!(!docs.truncated);
        assert_eq!(docs.total_bytes, 0);
        assert_eq!(docs.render(), None);
    }

    #[test]
    fn byte_cap_truncates_at_utf8_boundary() {
        let temp = TestDir::new("truncate");
        let root = temp.path();
        write(&root.join("AGENTS.md"), "abcédef");

        let docs = load_project_docs(root, root, &default_config(4));

        assert_eq!(docs.parts.len(), 1);
        assert_eq!(docs.parts[0].content, "abc");
        assert!(docs.truncated);
        assert_eq!(docs.total_bytes, 3);
    }

    #[test]
    fn non_utf8_file_returns_warning_without_panicking() {
        let temp = TestDir::new("non_utf8");
        let root = temp.path();
        fs::write(root.join("AGENTS.md"), [0xff, 0xfe]).expect("write invalid utf8");

        let docs = load_project_docs(root, root, &default_config(1024));

        assert!(docs.parts.is_empty());
        assert_eq!(docs.warnings.len(), 1);
        assert!(docs.warnings[0].message.contains("valid UTF-8"));
        assert!(docs.warnings[0].path.ends_with("AGENTS.md"));
        let rendered = docs.render().expect("warnings should render");
        assert!(rendered.contains("AGENTS.md instructions"));
        assert!(rendered.contains("Warnings"));
        assert!(rendered.contains("project doc is not valid UTF-8"));
    }

    #[test]
    fn render_includes_title_source_paths_and_content() {
        let temp = TestDir::new("render");
        let root = temp.path();
        write(&root.join("AGENTS.md"), "Use project rules.");

        let docs = load_project_docs(root, root, &default_config(1024));
        let rendered = docs.render().expect("rendered docs");

        assert!(rendered.contains("AGENTS.md instructions"));
        assert!(rendered.contains(&root.join("AGENTS.md").display().to_string()));
        assert!(rendered.contains("Use project rules."));
    }
}
