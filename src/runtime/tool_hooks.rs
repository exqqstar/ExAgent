use anyhow::Result;
use async_trait::async_trait;

use crate::session::ApprovalId;
use crate::tools::{ToolCapabilities, ToolOutcome, ToolRuntimeEffect};
use crate::types::ThreadId;

#[derive(Debug, Clone)]
pub(crate) struct ToolInvocationContext {
    pub(crate) invocation_id: String,
    pub(crate) tool_call_id: String,
    pub(crate) tool_name: String,
    #[allow(dead_code)]
    pub(crate) arguments: serde_json::Value,
    pub(crate) thread_id: Option<ThreadId>,
    pub(crate) workspace_root: std::path::PathBuf,
    pub(crate) capabilities: ToolCapabilities,
}

#[async_trait]
pub(crate) trait ToolHooks: Send + Sync {
    async fn before_invocation(
        &self,
        _ctx: &ToolInvocationContext,
    ) -> Result<Vec<ToolRuntimeEffect>>;

    async fn approval_requested(
        &self,
        _ctx: &ToolInvocationContext,
        _approval_id: &ApprovalId,
    ) -> Result<Vec<ToolRuntimeEffect>>;

    async fn before_handler_execution(
        &self,
        _ctx: &ToolInvocationContext,
    ) -> Result<Vec<ToolRuntimeEffect>>;

    async fn after_handler_completion(
        &self,
        _ctx: &ToolInvocationContext,
        _outcome: &ToolOutcome,
    ) -> Result<Vec<ToolRuntimeEffect>>;

    async fn failed(
        &self,
        _ctx: &ToolInvocationContext,
        _message: &str,
    ) -> Result<Vec<ToolRuntimeEffect>>;
}

#[derive(Debug, Default)]
pub(crate) struct NoopToolHooks;

#[async_trait]
impl ToolHooks for NoopToolHooks {
    async fn before_invocation(
        &self,
        _ctx: &ToolInvocationContext,
    ) -> Result<Vec<ToolRuntimeEffect>> {
        Ok(Vec::new())
    }

    async fn approval_requested(
        &self,
        _ctx: &ToolInvocationContext,
        _approval_id: &ApprovalId,
    ) -> Result<Vec<ToolRuntimeEffect>> {
        Ok(Vec::new())
    }

    async fn before_handler_execution(
        &self,
        _ctx: &ToolInvocationContext,
    ) -> Result<Vec<ToolRuntimeEffect>> {
        Ok(Vec::new())
    }

    async fn after_handler_completion(
        &self,
        _ctx: &ToolInvocationContext,
        _outcome: &ToolOutcome,
    ) -> Result<Vec<ToolRuntimeEffect>> {
        Ok(Vec::new())
    }

    async fn failed(
        &self,
        _ctx: &ToolInvocationContext,
        _message: &str,
    ) -> Result<Vec<ToolRuntimeEffect>> {
        Ok(Vec::new())
    }
}
