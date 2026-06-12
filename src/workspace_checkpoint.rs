use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};

static TEMP_INDEX_COUNTER: AtomicU64 = AtomicU64::new(1);
const DEFAULT_RETAIN_CHECKPOINTS: usize = 100;
const DEFAULT_MAX_CHECKPOINT_AGE_SECS: u64 = 60 * 60 * 24 * 30;

pub fn create_checkpoint(workspace_root: impl AsRef<Path>) -> Result<Option<String>> {
    let workspace_root = workspace_root
        .as_ref()
        .canonicalize()
        .with_context(|| "canonicalize workspace root for checkpoint")?;
    if !is_inside_git_work_tree(&workspace_root)? {
        return Ok(None);
    }

    let temp_index = TempIndex::new();
    let head = git_stdout_optional(&workspace_root, ["rev-parse", "--verify", "HEAD"])?;
    let ignored_manifest = ignored_manifest_bytes(&workspace_root)?;

    if head.is_some() {
        git_stdout_with_temp_index(&workspace_root, &temp_index.path, ["read-tree", "HEAD"])?;
    } else {
        git_stdout_with_temp_index(&workspace_root, &temp_index.path, ["read-tree", "--empty"])?;
    }
    git_stdout_with_temp_index(&workspace_root, &temp_index.path, ["add", "-A", "--", "."])?;
    let tree = git_stdout_with_temp_index(&workspace_root, &temp_index.path, ["write-tree"])?;
    let checkpoint_id = commit_tree(&workspace_root, &tree, head.as_deref())?;
    let checkpoint_ref = checkpoint_ref(&checkpoint_id)?;
    write_ignored_manifest(&workspace_root, &checkpoint_id, &ignored_manifest)?;
    git_stdout(
        &workspace_root,
        [
            "update-ref",
            checkpoint_ref.as_str(),
            checkpoint_id.as_str(),
        ],
    )?;
    if let Err(err) = prune_checkpoints(
        &workspace_root,
        DEFAULT_RETAIN_CHECKPOINTS,
        Some(Duration::from_secs(DEFAULT_MAX_CHECKPOINT_AGE_SECS)),
    ) {
        tracing::warn!(
            error = %err,
            workspace_root = %workspace_root.display(),
            "failed to prune workspace checkpoints"
        );
    }

    Ok(Some(checkpoint_id))
}

pub fn prune_checkpoints(
    workspace_root: impl AsRef<Path>,
    retain_count: usize,
    max_age: Option<Duration>,
) -> Result<()> {
    let workspace_root = workspace_root
        .as_ref()
        .canonicalize()
        .with_context(|| "canonicalize workspace root for checkpoint pruning")?;
    if !is_inside_git_work_tree(&workspace_root)? {
        return Ok(());
    }

    let mut refs = checkpoint_refs(&workspace_root)?;
    refs.sort_by(|left, right| {
        right
            .committer_timestamp
            .cmp(&left.committer_timestamp)
            .then_with(|| right.refname.cmp(&left.refname))
    });

    let now_secs = current_unix_secs();
    for (index, checkpoint_ref) in refs.iter().enumerate() {
        let over_count = index >= retain_count;
        let over_age = max_age
            .map(|age| now_secs.saturating_sub(checkpoint_ref.committer_timestamp) > age.as_secs())
            .unwrap_or(false);
        if over_count || over_age {
            delete_ref(&workspace_root, &checkpoint_ref.refname)?;
            if let Ok(manifest_ref) = ignored_manifest_ref(&checkpoint_ref.checkpoint_id) {
                delete_ref(&workspace_root, &manifest_ref)?;
            }
        }
    }

    Ok(())
}

pub fn restore_checkpoint(workspace_root: impl AsRef<Path>, checkpoint_id: &str) -> Result<()> {
    let workspace_root = workspace_root
        .as_ref()
        .canonicalize()
        .with_context(|| "canonicalize workspace root for checkpoint restore")?;
    if !is_inside_git_work_tree(&workspace_root)? {
        return Err(anyhow!("workspace is not inside a git work tree"));
    }

    let checkpoint_ref = checkpoint_ref(checkpoint_id)?;
    git_stdout(
        &workspace_root,
        [
            "rev-parse",
            "--verify",
            &format!("{checkpoint_ref}^{{commit}}"),
        ],
    )
    .with_context(|| "verify checkpoint ref")?;

    let checkpoint_paths = checkpoint_paths(&workspace_root, &checkpoint_ref)?;
    let ignored_manifest_paths = ignored_manifest_paths(&workspace_root, checkpoint_id)?;
    remove_paths_absent_from_checkpoint(
        &workspace_root,
        &checkpoint_paths,
        ignored_manifest_paths,
    )?;
    git_stdout(
        &workspace_root,
        [
            "restore",
            "--source",
            checkpoint_ref.as_str(),
            "--worktree",
            "--",
            ".",
        ],
    )?;

    Ok(())
}

fn is_inside_git_work_tree(cwd: &Path) -> Result<bool> {
    match git_stdout(cwd, ["rev-parse", "--is-inside-work-tree"]) {
        Ok(output) => Ok(output.trim() == "true"),
        Err(err) if is_git_not_repository_error(&err) => Ok(false),
        Err(err) => Err(err),
    }
}

fn is_git_not_repository_error(err: &anyhow::Error) -> bool {
    let message = format!("{err:#}");
    message.contains("not a git repository") || message.contains("not a git work tree")
}

fn checkpoint_ref(checkpoint_id: &str) -> Result<String> {
    if !is_hex_object_id(checkpoint_id) {
        return Err(anyhow!("invalid checkpoint id"));
    }
    Ok(format!("refs/exagent/checkpoints/{checkpoint_id}"))
}

fn ignored_manifest_ref(checkpoint_id: &str) -> Result<String> {
    if !is_hex_object_id(checkpoint_id) {
        return Err(anyhow!("invalid checkpoint id"));
    }
    Ok(format!("refs/exagent/checkpoint-manifests/{checkpoint_id}"))
}

fn is_hex_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug)]
struct CheckpointRef {
    refname: String,
    checkpoint_id: String,
    committer_timestamp: u64,
}

fn checkpoint_refs(cwd: &Path) -> Result<Vec<CheckpointRef>> {
    let output = git_stdout(
        cwd,
        [
            "for-each-ref",
            "--format=%(committerdate:unix)\t%(refname)",
            "refs/exagent/checkpoints",
        ],
    )?;
    let mut refs = Vec::new();
    for line in output.lines() {
        let Some((timestamp, refname)) = line.split_once('\t') else {
            continue;
        };
        let Some(checkpoint_id) = refname.strip_prefix("refs/exagent/checkpoints/") else {
            continue;
        };
        refs.push(CheckpointRef {
            refname: refname.to_string(),
            checkpoint_id: checkpoint_id.to_string(),
            committer_timestamp: timestamp.parse().unwrap_or(0),
        });
    }
    Ok(refs)
}

fn delete_ref(cwd: &Path, refname: &str) -> Result<()> {
    git_stdout(cwd, ["update-ref", "-d", refname])?;
    Ok(())
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn commit_tree(cwd: &Path, tree: &str, parent: Option<&str>) -> Result<String> {
    let mut command = git_command(cwd);
    command
        .arg("commit-tree")
        .arg(tree.trim())
        .arg("-m")
        .arg("ExAgent workspace checkpoint")
        .env("GIT_AUTHOR_NAME", "ExAgent")
        .env("GIT_AUTHOR_EMAIL", "exagent@example.invalid")
        .env("GIT_COMMITTER_NAME", "ExAgent")
        .env("GIT_COMMITTER_EMAIL", "exagent@example.invalid");
    if let Some(parent) = parent {
        command.arg("-p").arg(parent.trim());
    }
    successful_stdout(command)
        .map(|output| output.trim().to_string())
        .and_then(|checkpoint_id| {
            if is_hex_object_id(&checkpoint_id) {
                Ok(checkpoint_id)
            } else {
                Err(anyhow!("git commit-tree returned invalid checkpoint id"))
            }
        })
}

fn checkpoint_paths(workspace_root: &Path, checkpoint_ref: &str) -> Result<HashSet<PathBuf>> {
    let repo_root = git_stdout(workspace_root, ["rev-parse", "--show-toplevel"])?;
    let repo_root = PathBuf::from(repo_root.trim())
        .canonicalize()
        .with_context(|| "canonicalize git repository root")?;
    let workspace_prefix = workspace_root
        .strip_prefix(&repo_root)
        .with_context(|| "workspace root is outside git repository root")?;
    let pathspec = git_pathspec(workspace_prefix);
    let output = checked_git_output(&repo_root, ls_tree_args(checkpoint_ref, pathspec))?;
    let mut paths = HashSet::new();
    for raw_path in output.stdout.split(|byte| *byte == 0) {
        if raw_path.is_empty() {
            continue;
        }
        let repo_relative = pathbuf_from_git_bytes(raw_path);
        let workspace_relative = if workspace_prefix.as_os_str().is_empty() {
            repo_relative
        } else {
            repo_relative
                .strip_prefix(workspace_prefix)
                .with_context(|| "checkpoint path escaped workspace root")?
                .to_path_buf()
        };
        if !workspace_relative.as_os_str().is_empty() {
            paths.insert(workspace_relative);
        }
    }
    Ok(paths)
}

fn git_pathspec(path: &Path) -> OsString {
    if path.as_os_str().is_empty() {
        OsString::from(".")
    } else {
        OsString::from(format!(
            ":(literal){}",
            path.to_string_lossy().replace('\\', "/")
        ))
    }
}

fn ls_tree_args(checkpoint_ref: &str, pathspec: OsString) -> Vec<OsString> {
    vec![
        OsString::from("ls-tree"),
        OsString::from("-rz"),
        OsString::from("--name-only"),
        OsString::from("-r"),
        OsString::from(checkpoint_ref),
        OsString::from("--"),
        pathspec,
    ]
}

fn remove_paths_absent_from_checkpoint(
    workspace_root: &Path,
    checkpoint_paths: &HashSet<PathBuf>,
    ignored_manifest_paths: Option<HashSet<PathBuf>>,
) -> Result<()> {
    let mut entries = Vec::new();
    let gitlinks = gitlinks_under_workspace(workspace_root)?;
    collect_workspace_entries(workspace_root, workspace_root, &gitlinks, &mut entries)?;
    let current_ignored_paths = if ignored_manifest_paths.is_none() {
        ignored_paths_for_entries(workspace_root, &entries)?
    } else {
        HashSet::new()
    };
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.relative.components().count()));

    for entry in entries {
        let path = entry.path;
        let relative = entry.relative;
        if !path.starts_with(workspace_root) {
            return Err(anyhow!("refusing to remove path outside workspace root"));
        }
        if path.file_name() == Some(OsStr::new(".git")) {
            continue;
        }
        if entry.is_dir {
            if should_preserve_ignored_path(
                &relative,
                ignored_manifest_paths.as_ref(),
                &current_ignored_paths,
            ) {
                continue;
            }
            if fs::read_dir(&path)?.next().is_none() {
                fs::remove_dir(&path)
                    .with_context(|| format!("remove empty directory {}", path.display()))?;
            }
            continue;
        }

        if !checkpoint_paths.contains(&relative)
            && !should_preserve_ignored_path(
                &relative,
                ignored_manifest_paths.as_ref(),
                &current_ignored_paths,
            )
        {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
    }

    Ok(())
}

fn should_preserve_ignored_path(
    relative: &Path,
    ignored_manifest_paths: Option<&HashSet<PathBuf>>,
    current_ignored_paths: &HashSet<PathBuf>,
) -> bool {
    match ignored_manifest_paths {
        Some(paths) => paths.contains(relative),
        None => current_ignored_paths.contains(relative),
    }
}

fn ignored_manifest_bytes(workspace_root: &Path) -> Result<Vec<u8>> {
    let gitlinks = gitlinks_under_workspace(workspace_root)?;
    let mut entries = Vec::new();
    collect_workspace_entries(workspace_root, workspace_root, &gitlinks, &mut entries)?;
    let mut paths = ignored_paths_for_entries(workspace_root, &entries)?
        .into_iter()
        .collect::<Vec<_>>();
    paths.sort();

    let mut manifest = Vec::new();
    for path in paths {
        manifest.extend(path_to_manifest_bytes(&path));
        manifest.push(0);
    }
    Ok(manifest)
}

fn write_ignored_manifest(
    workspace_root: &Path,
    checkpoint_id: &str,
    manifest: &[u8],
) -> Result<()> {
    let blob_id =
        git_stdout_with_stdin(workspace_root, ["hash-object", "-w", "--stdin"], manifest)?;
    let manifest_ref = ignored_manifest_ref(checkpoint_id)?;
    git_stdout(
        workspace_root,
        ["update-ref", manifest_ref.as_str(), blob_id.trim()],
    )?;
    Ok(())
}

fn ignored_manifest_paths(
    workspace_root: &Path,
    checkpoint_id: &str,
) -> Result<Option<HashSet<PathBuf>>> {
    let manifest_ref = ignored_manifest_ref(checkpoint_id)?;
    let output = git_output(workspace_root, ["cat-file", "blob", manifest_ref.as_str()])?;
    if !output.status.success() {
        return Ok(None);
    }

    let mut paths = HashSet::new();
    for raw_path in output.stdout.split(|byte| *byte == 0) {
        if raw_path.is_empty() {
            continue;
        }
        paths.insert(pathbuf_from_git_bytes(raw_path));
    }
    Ok(Some(paths))
}

#[derive(Debug, Clone)]
struct WorkspaceEntry {
    path: PathBuf,
    relative: PathBuf,
    is_dir: bool,
}

fn collect_workspace_entries(
    root: &Path,
    workspace_root: &Path,
    gitlinks: &HashSet<PathBuf>,
    entries: &mut Vec<WorkspaceEntry>,
) -> Result<()> {
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.file_name() == Some(OsStr::new(".git")) {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("read metadata for {}", path.display()))?;
        let relative = path
            .strip_prefix(workspace_root)
            .with_context(|| "workspace entry escaped root")?
            .to_path_buf();
        let is_dir = metadata.is_dir();
        if is_dir {
            if is_nested_git_dir(&path) || gitlinks.contains(&relative) {
                continue;
            }
            collect_workspace_entries(&path, workspace_root, gitlinks, entries)?;
        }
        entries.push(WorkspaceEntry {
            path,
            relative,
            is_dir,
        });
    }
    Ok(())
}

fn is_nested_git_dir(path: &Path) -> bool {
    path.join(".git").exists()
}

fn gitlinks_under_workspace(workspace_root: &Path) -> Result<HashSet<PathBuf>> {
    let repo_root = git_stdout(workspace_root, ["rev-parse", "--show-toplevel"])?;
    let repo_root = PathBuf::from(repo_root.trim())
        .canonicalize()
        .with_context(|| "canonicalize git repository root")?;
    let workspace_prefix = workspace_root
        .strip_prefix(&repo_root)
        .with_context(|| "workspace root is outside git repository root")?;
    let output = checked_git_output(
        &repo_root,
        git_ls_files_args("-s", git_pathspec(workspace_prefix)),
    )?;
    let mut gitlinks = HashSet::new();
    for record in output.stdout.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let Some(tab_index) = record.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        if !record[..tab_index].starts_with(b"160000 ") {
            continue;
        }
        let repo_relative = pathbuf_from_git_bytes(&record[tab_index + 1..]);
        let workspace_relative = if workspace_prefix.as_os_str().is_empty() {
            repo_relative
        } else {
            repo_relative
                .strip_prefix(workspace_prefix)
                .with_context(|| "gitlink path escaped workspace root")?
                .to_path_buf()
        };
        gitlinks.insert(workspace_relative);
    }
    Ok(gitlinks)
}

fn ignored_paths_for_entries(
    workspace_root: &Path,
    entries: &[WorkspaceEntry],
) -> Result<HashSet<PathBuf>> {
    if entries.is_empty() {
        return Ok(HashSet::new());
    }

    let mut stdin = Vec::new();
    for entry in entries {
        stdin.extend(path_to_manifest_bytes(&entry.relative));
        stdin.push(0);
    }

    let output = git_output_with_stdin(workspace_root, ["check-ignore", "-z", "--stdin"], &stdin)?;
    match output.status.code() {
        Some(0) | Some(1) => {
            let mut ignored = HashSet::new();
            for raw_path in output.stdout.split(|byte| *byte == 0) {
                if raw_path.is_empty() {
                    continue;
                }
                ignored.insert(pathbuf_from_git_bytes(raw_path));
            }
            Ok(ignored)
        }
        _ => Err(git_status_error(output)),
    }
}

fn git_ls_files_args(flag: &str, pathspec: OsString) -> Vec<OsString> {
    vec![
        OsString::from("ls-files"),
        OsString::from("-z"),
        OsString::from(flag),
        OsString::from("--"),
        pathspec,
    ]
}

fn git_stdout_optional<I, S>(cwd: &Path, args: I) -> Result<Option<String>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = git_output(cwd, args)?;
    if output.status.success() {
        return Ok(Some(String::from_utf8(output.stdout)?.trim().to_string()));
    }
    Ok(None)
}

fn git_stdout<I, S>(cwd: &Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    stdout_from_output(git_output(cwd, args)?)
}

fn git_stdout_with_temp_index<I, S>(cwd: &Path, index_path: &Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = git_command(cwd);
    command.env("GIT_INDEX_FILE", index_path);
    command.args(args);
    successful_stdout(command)
}

fn git_stdout_with_stdin<I, S>(cwd: &Path, args: I, stdin: &[u8]) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    stdout_from_output(git_output_with_stdin(cwd, args, stdin)?)
}

fn git_output_with_stdin<I, S>(cwd: &Path, args: I, stdin: &[u8]) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut child = {
        let mut command = git_command(cwd);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.spawn().with_context(|| "run git command")?
    };
    let mut stdin_handle = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("git stdin unavailable"))?;
    stdin_handle
        .write_all(stdin)
        .with_context(|| "write git stdin")?;
    drop(stdin_handle);
    child.wait_with_output().with_context(|| "wait for git")
}

fn git_output<I, S>(cwd: &Path, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    git_command_with_args(cwd, args)
}

fn checked_git_output<I, S>(cwd: &Path, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = git_output(cwd, args)?;
    if output.status.success() {
        return Ok(output);
    }
    Err(git_status_error(output))
}

fn git_command_with_args<I, S>(cwd: &Path, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = git_command(cwd);
    command.args(args);
    command
        .output()
        .with_context(|| format!("run git in {}", cwd.display()))
}

fn stdout_from_output(output: Output) -> Result<String> {
    if output.status.success() {
        return Ok(String::from_utf8(output.stdout)?);
    }
    Err(git_status_error(output))
}

fn git_status_error(output: Output) -> anyhow::Error {
    anyhow!(
        "git command failed: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn successful_stdout(mut command: Command) -> Result<String> {
    let output = command.output().with_context(|| "run git command")?;
    stdout_from_output(output)
}

fn git_command(cwd: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(cwd);
    command
}

fn pathbuf_from_git_bytes(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(OsString::from_vec(bytes.to_vec()))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

fn path_to_manifest_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        path.to_string_lossy().as_bytes().to_vec()
    }
}

struct TempIndex {
    path: PathBuf,
}

impl TempIndex {
    fn new() -> Self {
        let counter = TEMP_INDEX_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "exagent-checkpoint-{}-{nanos}-{counter}.index",
            std::process::id()
        ));
        Self { path }
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        if let Some(file_name) = self.path.file_name() {
            let mut lock_name = file_name.to_os_string();
            lock_name.push(".lock");
            let _ = fs::remove_file(self.path.with_file_name(lock_name));
        }
    }
}
