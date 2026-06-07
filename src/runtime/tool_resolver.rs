use std::sync::Arc;

use crate::registry::ToolRegistry;
use crate::tools::ToolHandler;
use crate::types::ToolCall;

#[derive(Clone)]
pub(crate) struct ToolResolver {
    registry: Arc<ToolRegistry>,
}

impl ToolResolver {
    pub(crate) fn new(registry: ToolRegistry) -> Self {
        Self {
            registry: Arc::new(registry),
        }
    }

    pub(crate) fn resolve(&self, call: &ToolCall) -> Option<Arc<dyn ToolHandler>> {
        self.registry.handler(&call.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::read_file::ReadFileTool;
    use crate::types::ToolCall;

    #[test]
    fn tool_resolver_resolves_registered_handlers() {
        let mut registry = ToolRegistry::new();
        registry.register(ReadFileTool);
        let resolver = ToolResolver::new(registry);
        let call = ToolCall {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({ "path": "notes.txt" }),
            thought_signature: None,
        };

        let handler = resolver.resolve(&call).expect("read_file handler");
        assert_eq!(handler.spec().name, "read_file");
    }
}
