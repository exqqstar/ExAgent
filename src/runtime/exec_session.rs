use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;

use crate::events::ExecOutputStream;
use crate::session::{ExecSessionId, ExecSessionStatus};
use crate::types::ThreadId;

static EXEC_SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Default)]
pub struct ExecSessionManager {
    sessions: Arc<Mutex<HashMap<String, HashMap<String, Arc<ActiveExecSession>>>>>,
}

#[derive(Debug, Clone)]
pub struct ExecSessionSnapshot {
    pub exec_session_id: ExecSessionId,
    pub command: String,
    pub cwd: PathBuf,
    pub status: ExecSessionStatus,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

struct ActiveExecSession {
    thread_id: ThreadId,
    exec_session_id: ExecSessionId,
    command: String,
    cwd: PathBuf,
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    state: Mutex<ExecSessionState>,
}

#[derive(Default)]
struct ExecSessionState {
    stdout: String,
    stderr: String,
    status: Option<ExecSessionStatus>,
    exit_code: Option<i32>,
}

impl ExecSessionManager {
    pub async fn start(
        &self,
        _workspace_root: &Path,
        thread_id: &ThreadId,
        command: &str,
        cwd: PathBuf,
    ) -> Result<ExecSessionSnapshot, String> {
        let mut child = Command::new("sh")
            .arg("-lc")
            .arg(command)
            .current_dir(&cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| err.to_string())?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "failed to capture stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to capture stdout".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "failed to capture stderr".to_string())?;

        let handle = Arc::new(ActiveExecSession {
            thread_id: thread_id.clone(),
            exec_session_id: new_exec_session_id(),
            command: command.to_string(),
            cwd: cwd.clone(),
            child: Mutex::new(child),
            stdin: Mutex::new(Some(stdin)),
            state: Mutex::new(ExecSessionState {
                status: Some(ExecSessionStatus::Running),
                ..ExecSessionState::default()
            }),
        });

        self.insert_handle(handle.clone()).await;
        spawn_output_task(handle.clone(), ExecOutputStream::Stdout, stdout);
        spawn_output_task(handle.clone(), ExecOutputStream::Stderr, stderr);

        self.snapshot(handle).await
    }

    pub async fn write_stdin(
        &self,
        thread_id: &ThreadId,
        exec_session_id: &ExecSessionId,
        input: &str,
    ) -> Result<ExecSessionSnapshot, String> {
        let handle = self.get_handle(thread_id, exec_session_id).await?;
        self.refresh_status(&handle).await?;

        {
            let state = handle.state.lock().await;
            if !matches!(state.status, Some(ExecSessionStatus::Running)) {
                return Err("exec session is not running".into());
            }
        }

        {
            let mut stdin_guard = handle.stdin.lock().await;
            let stdin = stdin_guard
                .as_mut()
                .ok_or_else(|| "stdin is closed for this exec session".to_string())?;
            stdin
                .write_all(input.as_bytes())
                .await
                .map_err(|err| err.to_string())?;
            stdin.flush().await.map_err(|err| err.to_string())?;
        }

        self.snapshot(handle).await
    }

    pub async fn poll(
        &self,
        thread_id: &ThreadId,
        exec_session_id: &ExecSessionId,
    ) -> Result<ExecSessionSnapshot, String> {
        let handle = self.get_handle(thread_id, exec_session_id).await?;
        self.snapshot(handle).await
    }

    pub async fn terminate(
        &self,
        thread_id: &ThreadId,
        exec_session_id: &ExecSessionId,
    ) -> Result<ExecSessionSnapshot, String> {
        let handle = self.get_handle(thread_id, exec_session_id).await?;
        {
            let mut child = handle.child.lock().await;
            if child.try_wait().map_err(|err| err.to_string())?.is_none() {
                child.kill().await.map_err(|err| err.to_string())?;
            }
        }

        {
            let mut stdin = handle.stdin.lock().await;
            stdin.take();
        }

        {
            let mut state = handle.state.lock().await;
            state.status = Some(ExecSessionStatus::Terminated);
            state.exit_code = None;
        }

        self.snapshot(handle).await
    }

    async fn insert_handle(&self, handle: Arc<ActiveExecSession>) {
        let mut sessions = self.sessions.lock().await;
        sessions
            .entry(handle.thread_id.as_str().to_string())
            .or_default()
            .insert(handle.exec_session_id.as_str().to_string(), handle);
    }

    async fn get_handle(
        &self,
        thread_id: &ThreadId,
        exec_session_id: &ExecSessionId,
    ) -> Result<Arc<ActiveExecSession>, String> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(thread_id.as_str())
            .and_then(|entries| entries.get(exec_session_id.as_str()))
            .cloned()
            .ok_or_else(|| format!("unknown exec session: {}", exec_session_id.as_str()))
    }

    async fn snapshot(
        &self,
        handle: Arc<ActiveExecSession>,
    ) -> Result<ExecSessionSnapshot, String> {
        self.refresh_status(&handle).await?;
        let state = handle.state.lock().await;
        Ok(ExecSessionSnapshot {
            exec_session_id: handle.exec_session_id.clone(),
            command: handle.command.clone(),
            cwd: handle.cwd.clone(),
            status: state.status.clone().unwrap_or(ExecSessionStatus::Running),
            stdout: state.stdout.clone(),
            stderr: state.stderr.clone(),
            exit_code: state.exit_code,
        })
    }

    async fn refresh_status(&self, handle: &Arc<ActiveExecSession>) -> Result<(), String> {
        let wait_status = {
            let mut child = handle.child.lock().await;
            child.try_wait().map_err(|err| err.to_string())?
        };

        if let Some(status) = wait_status {
            let mut state = handle.state.lock().await;
            if matches!(state.status, Some(ExecSessionStatus::Running)) {
                state.status = Some(ExecSessionStatus::Exited);
                state.exit_code = status.code();
            }
        }

        Ok(())
    }
}

fn spawn_output_task<R>(handle: Arc<ActiveExecSession>, stream: ExecOutputStream, reader: R)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut reader = reader;
        let mut buf = [0_u8; 1024];
        loop {
            let read = match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(read) => read,
                Err(_) => break,
            };

            let chunk = String::from_utf8_lossy(&buf[..read]).to_string();
            {
                let mut state = handle.state.lock().await;
                match stream {
                    ExecOutputStream::Stdout => state.stdout.push_str(&chunk),
                    ExecOutputStream::Stderr => state.stderr.push_str(&chunk),
                }
            }
        }
    });
}

fn new_exec_session_id() -> ExecSessionId {
    let next = EXEC_SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    ExecSessionId::new(format!("exec_{next}"))
}
