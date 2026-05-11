use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex, MutexGuard};

use crate::config::AgentConfig;
use crate::policy::PolicyMode;
use crate::session::AgentRole;
use crate::types::{SessionId, TurnId};

static TURN_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserInput {
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnContext {
    pub model: String,
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub policy_mode: PolicyMode,
    pub agent_role: AgentRole,
    pub instructions: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnContextRequest {
    pub workspace_root: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub policy_mode: Option<PolicyMode>,
    pub agent_role: Option<AgentRole>,
    pub instructions: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadStartRequest {
    pub context: TurnContextRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadStartResult {
    pub session_id: SessionId,
    pub status: String,
    pub context: TurnContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnStartRequest {
    pub session_id: SessionId,
    pub input: Vec<UserInput>,
    pub context: TurnContextRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnStartResult {
    pub turn_id: TurnId,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeExecution {
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeOp {
    UserInput {
        turn_id: TurnId,
        input: Vec<UserInput>,
        context: TurnContext,
    },
    Interrupt {
        turn_id: Option<TurnId>,
    },
    Compact,
    Shutdown,
    SetThreadName {
        name: String,
    },
}

#[derive(Debug, Clone)]
pub struct ConfigManager {
    base: AgentConfig,
}

impl ConfigManager {
    pub fn new(base: AgentConfig) -> Self {
        Self { base }
    }

    pub fn build_turn_context(&self, request: TurnContextRequest) -> Result<TurnContext> {
        let workspace_overridden = request.workspace_root.is_some();
        let workspace_root = match request.workspace_root.as_deref() {
            Some(raw) => canonicalize_from_current(raw)?,
            None => std::fs::canonicalize(&self.base.workspace_root).with_context(|| {
                format!(
                    "workspace_root does not exist or is not accessible: {}",
                    self.base.workspace_root.display()
                )
            })?,
        };

        let cwd = match request.cwd.as_deref() {
            Some(raw) => canonicalize_from_root(&workspace_root, raw)?,
            None if workspace_overridden => workspace_root.clone(),
            None => canonicalize_existing_cwd(&workspace_root, &self.base.cwd)?,
        };

        Ok(TurnContext {
            model: request.model.unwrap_or_else(|| self.base.model.clone()),
            workspace_root,
            cwd,
            policy_mode: request.policy_mode.unwrap_or(self.base.policy_mode),
            agent_role: request.agent_role.unwrap_or_default(),
            instructions: request.instructions,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedThreadStatus {
    Idle,
    Running,
    Archived,
}

#[derive(Clone, Default)]
pub struct ThreadManager {
    live_threads: Arc<Mutex<HashMap<String, ThreadHandle>>>,
}

#[derive(Clone)]
pub struct RuntimeController {
    config: ConfigManager,
    threads: ThreadManager,
    thread_defaults: Arc<Mutex<HashMap<String, TurnContext>>>,
}

pub struct RuntimeEngine<E> {
    executor: E,
}

#[async_trait]
pub trait RuntimeOpExecutor: Send + Sync {
    async fn execute_op(&self, session_id: &SessionId, op: RuntimeOp) -> Result<RuntimeExecution>;
}

#[derive(Clone)]
pub struct ThreadHandle {
    inner: Arc<ThreadHandleInner>,
}

struct ThreadHandleInner {
    session_id: SessionId,
    op_tx: mpsc::Sender<RuntimeOp>,
    op_rx: Mutex<mpsc::Receiver<RuntimeOp>>,
    status: Mutex<ManagedThreadStatus>,
    execution_lock: Mutex<()>,
}

pub struct ThreadExecutionGuard<'a> {
    _guard: MutexGuard<'a, ()>,
}

impl ThreadManager {
    pub async fn get_or_start(&self, session_id: SessionId) -> ThreadHandle {
        let mut live_threads = self.live_threads.lock().await;
        if let Some(handle) = live_threads.get(session_id.as_str()) {
            return handle.clone();
        }

        let handle = ThreadHandle::new(session_id.clone());
        live_threads.insert(session_id.as_str().to_string(), handle.clone());
        handle
    }

    pub async fn live_thread_count(&self) -> usize {
        self.live_threads.lock().await.len()
    }
}

impl RuntimeController {
    pub fn new(base: AgentConfig) -> Self {
        Self {
            config: ConfigManager::new(base),
            threads: ThreadManager::default(),
            thread_defaults: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn start_thread(&self, request: ThreadStartRequest) -> Result<ThreadStartResult> {
        let context = self.config.build_turn_context(request.context)?;
        let session_id = crate::transcript::new_session_id();
        self.threads.get_or_start(session_id.clone()).await;
        self.thread_defaults
            .lock()
            .await
            .insert(session_id.as_str().to_string(), context.clone());

        Ok(ThreadStartResult {
            session_id,
            status: "idle".into(),
            context,
        })
    }

    pub async fn start_turn(&self, request: TurnStartRequest) -> Result<TurnStartResult> {
        if request.input.is_empty() {
            bail!("turn input cannot be empty");
        }

        let context = self.turn_context_for_request(&request).await?;
        let handle = self.threads.get_or_start(request.session_id.clone()).await;
        let turn_id = new_turn_id();
        handle
            .submit(RuntimeOp::UserInput {
                turn_id: turn_id.clone(),
                input: request.input,
                context,
            })
            .await?;

        Ok(TurnStartResult {
            turn_id,
            status: "queued".into(),
        })
    }

    pub async fn thread_handle(&self, session_id: &SessionId) -> Option<ThreadHandle> {
        self.threads.get_live(session_id).await
    }

    async fn turn_context_for_request(&self, request: &TurnStartRequest) -> Result<TurnContext> {
        let default_context = self
            .thread_defaults
            .lock()
            .await
            .get(request.session_id.as_str())
            .cloned();

        let Some(default_context) = default_context else {
            return self.config.build_turn_context(request.context.clone());
        };

        if request.context == TurnContextRequest::default() {
            return Ok(default_context);
        }

        ConfigManager::new(config_from_context(&default_context))
            .build_turn_context(request.context.clone())
    }
}

impl<E> RuntimeEngine<E>
where
    E: RuntimeOpExecutor,
{
    pub fn new(executor: E) -> Self {
        Self { executor }
    }

    pub async fn run_next(&self, handle: &ThreadHandle) -> Result<Option<RuntimeExecution>> {
        let Some(op) = handle.next_op().await else {
            return Ok(None);
        };

        handle.set_status(ManagedThreadStatus::Running).await;
        let result = self.executor.execute_op(handle.session_id(), op).await;
        handle.set_status(ManagedThreadStatus::Idle).await;
        result.map(Some)
    }
}

impl ThreadManager {
    pub async fn get_live(&self, session_id: &SessionId) -> Option<ThreadHandle> {
        self.live_threads
            .lock()
            .await
            .get(session_id.as_str())
            .cloned()
    }
}

impl ThreadHandle {
    fn new(session_id: SessionId) -> Self {
        let (op_tx, op_rx) = mpsc::channel(64);
        Self {
            inner: Arc::new(ThreadHandleInner {
                session_id,
                op_tx,
                op_rx: Mutex::new(op_rx),
                status: Mutex::new(ManagedThreadStatus::Idle),
                execution_lock: Mutex::new(()),
            }),
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.inner.session_id
    }

    pub fn same_thread(&self, other: &ThreadHandle) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub async fn submit(&self, op: RuntimeOp) -> Result<()> {
        if self.status().await == ManagedThreadStatus::Archived {
            bail!("cannot submit op to archived thread");
        }

        self.inner
            .op_tx
            .send(op)
            .await
            .context("thread op queue is closed")
    }

    pub async fn next_op(&self) -> Option<RuntimeOp> {
        self.inner.op_rx.lock().await.recv().await
    }

    pub async fn status(&self) -> ManagedThreadStatus {
        *self.inner.status.lock().await
    }

    pub async fn set_status(&self, status: ManagedThreadStatus) {
        *self.inner.status.lock().await = status;
    }

    pub async fn lock_execution(&self) -> ThreadExecutionGuard<'_> {
        ThreadExecutionGuard {
            _guard: self.inner.execution_lock.lock().await,
        }
    }
}

fn canonicalize_from_current(raw: &str) -> Result<PathBuf> {
    let path = PathBuf::from(raw);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .context("Failed to resolve current directory")?
            .join(path)
    };

    std::fs::canonicalize(&path).with_context(|| {
        format!(
            "workspace_root does not exist or is not accessible: {}",
            path.display()
        )
    })
}

fn canonicalize_existing_cwd(workspace_root: &Path, cwd: &Path) -> Result<PathBuf> {
    let raw = cwd
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("cwd must be valid UTF-8"))?;
    canonicalize_from_root(workspace_root, raw)
}

fn config_from_context(context: &TurnContext) -> AgentConfig {
    AgentConfig {
        model: context.model.clone(),
        workspace_root: context.workspace_root.clone(),
        cwd: context.cwd.clone(),
        policy_mode: context.policy_mode,
        ..AgentConfig::default()
    }
}

fn new_turn_id() -> TurnId {
    let next = TURN_COUNTER.fetch_add(1, Ordering::Relaxed);
    TurnId::new(format!("turn_{next}"))
}

fn canonicalize_from_root(root: &Path, raw: &str) -> Result<PathBuf> {
    let path = PathBuf::from(raw);
    let candidate = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };

    let candidate = std::fs::canonicalize(&candidate).with_context(|| {
        format!(
            "cwd does not exist or is not accessible: {}",
            candidate.display()
        )
    })?;

    if !candidate.starts_with(root) {
        bail!("cwd must stay within workspace_root");
    }

    Ok(candidate)
}
