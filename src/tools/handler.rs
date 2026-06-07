use async_trait::async_trait;

use crate::registry::ToolContext;
use crate::tools::outcome::ToolOutcome;
use crate::tools::spec::ToolSpec;
use crate::types::ToolCall;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCapabilities {
    pub mutating: bool,
    pub requires_approval: bool,
    pub parallel_safe: bool,
}

impl ToolCapabilities {
    pub fn read_only() -> Self {
        Self {
            mutating: false,
            requires_approval: false,
            parallel_safe: true,
        }
    }

    pub fn mutating(requires_approval: bool) -> Self {
        Self {
            mutating: true,
            requires_approval,
            parallel_safe: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolInvocation {
    pub invocation_id: String,
    pub call: ToolCall,
}

#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn spec(&self) -> ToolSpec;
    fn capabilities(&self) -> ToolCapabilities;
    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome;
}
