use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq)]
pub enum ToolSpecKind {
    Function { input_schema: Value },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub kind: ToolSpecKind,
}

impl ToolSpec {
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            kind: ToolSpecKind::Function { input_schema },
        }
    }

    pub fn to_internal_schema(&self) -> Value {
        match &self.kind {
            ToolSpecKind::Function { input_schema } => json!({
                "name": self.name.clone(),
                "description": self.description.clone(),
                "input_schema": input_schema.clone(),
            }),
        }
    }
}
