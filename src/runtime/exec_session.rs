use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;

use crate::events::ExecOutputStream;
use crate::runtime::process_cleanup::{
    cleanup_child_process_tree, configure_process_group, ProcessCleanupReason,
};
use crate::session::{ExecSessionId, ExecSessionStatus};
use crate::types::{ThreadId, TurnId};

static EXEC_SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct ExecOutputEventSink {
    emit: Arc<dyn Fn(ExecOutputEvent) + Send + Sync>,
}

impl ExecOutputEventSink {
    pub(crate) fn new(emit: impl Fn(ExecOutputEvent) + Send + Sync + 'static) -> Self {
        Self {
            emit: Arc::new(emit),
        }
    }

    fn emit(&self, event: ExecOutputEvent) {
        (self.emit)(event);
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ExecOutputEvent {
    pub(crate) turn_id: Option<TurnId>,
    pub(crate) invocation_id: Option<String>,
    pub(crate) exec_session_id: ExecSessionId,
    pub(crate) stream: ExecOutputStream,
    pub(crate) chunk: String,
    pub(crate) sequence: u64,
}

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
    pub stdout_delta: String,
    pub stderr_delta: String,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub stdout_delta_bytes: usize,
    pub stderr_delta_bytes: usize,
    pub output_sequence: u64,
    pub exit_code: Option<i32>,
}

struct ActiveExecSession {
    thread_id: ThreadId,
    turn_id: Option<TurnId>,
    invocation_id: Option<String>,
    exec_session_id: ExecSessionId,
    command: String,
    cwd: PathBuf,
    output_sink: Option<ExecOutputEventSink>,
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    state: Mutex<ExecSessionState>,
}

#[derive(Default)]
struct ExecSessionState {
    stdout: String,
    stderr: String,
    stdout_cursor: usize,
    stderr_cursor: usize,
    output_sequence: u64,
    status: Option<ExecSessionStatus>,
    exit_code: Option<i32>,
}

impl ExecSessionManager {
    pub async fn start(
        &self,
        _workspace_root: &Path,
        thread_id: &ThreadId,
        turn_id: Option<TurnId>,
        invocation_id: Option<String>,
        command: &str,
        cwd: PathBuf,
        output_sink: Option<ExecOutputEventSink>,
    ) -> Result<ExecSessionSnapshot, String> {
        let mut command_builder = Command::new("sh");
        command_builder
            .arg("-lc")
            .arg(command)
            .current_dir(&cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        configure_process_group(&mut command_builder);

        let mut child = command_builder.spawn().map_err(|err| err.to_string())?;

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
            turn_id,
            invocation_id,
            exec_session_id: new_exec_session_id(),
            command: command.to_string(),
            cwd: cwd.clone(),
            output_sink,
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
                cleanup_child_process_tree(
                    &mut child,
                    ProcessCleanupReason::Terminate,
                    Duration::from_millis(750),
                )
                .await;
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
        let mut state = handle.state.lock().await;
        let stdout_bytes = state.stdout.len();
        let stderr_bytes = state.stderr.len();
        let stdout_cursor = state.stdout_cursor.min(stdout_bytes);
        let stderr_cursor = state.stderr_cursor.min(stderr_bytes);
        let stdout_delta = state.stdout[stdout_cursor..].to_string();
        let stderr_delta = state.stderr[stderr_cursor..].to_string();
        let stdout_delta_bytes = stdout_delta.len();
        let stderr_delta_bytes = stderr_delta.len();
        state.stdout_cursor = stdout_bytes;
        state.stderr_cursor = stderr_bytes;
        Ok(ExecSessionSnapshot {
            exec_session_id: handle.exec_session_id.clone(),
            command: handle.command.clone(),
            cwd: handle.cwd.clone(),
            status: state.status.clone().unwrap_or(ExecSessionStatus::Running),
            stdout: state.stdout.clone(),
            stderr: state.stderr.clone(),
            stdout_delta,
            stderr_delta,
            stdout_bytes,
            stderr_bytes,
            stdout_delta_bytes,
            stderr_delta_bytes,
            output_sequence: state.output_sequence,
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
            let sequence = {
                let mut state = handle.state.lock().await;
                match stream {
                    ExecOutputStream::Stdout => state.stdout.push_str(&chunk),
                    ExecOutputStream::Stderr => state.stderr.push_str(&chunk),
                }
                state.output_sequence = state.output_sequence.saturating_add(1);
                state.output_sequence
            };
            if let Some(sink) = &handle.output_sink {
                sink.emit(ExecOutputEvent {
                    turn_id: handle.turn_id.clone(),
                    invocation_id: handle.invocation_id.clone(),
                    exec_session_id: handle.exec_session_id.clone(),
                    stream: stream.clone(),
                    chunk,
                    sequence,
                });
            }
        }
    });
}

fn new_exec_session_id() -> ExecSessionId {
    let next = EXEC_SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    ExecSessionId::new(format!("exec_{next}"))
}
