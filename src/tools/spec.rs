use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq)]
pub enum ToolSpecKind {
    Function { input_schema: Value },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// Strict structured-calling flag. Stored on the spec for tools that opt in;
    /// it is an internal contract today and is not serialized onto provider
    /// requests. See ADR-0042.
    pub strict: bool,
    /// Declared shape of the tool's result. Internal contract only: used for
    /// validation, future code-mode field access, and self-documentation. It is
    /// intentionally NOT sent on any provider request (the Anthropic tools wire
    /// protocol has no output_schema field). See ADR-0042.
    pub output_schema: Option<Value>,
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
            strict: false,
            output_schema: None,
            kind: ToolSpecKind::Function { input_schema },
        }
    }

    /// Declare the tool's output shape. Internal contract only; not sent on the
    /// wire (see ADR-0042).
    pub fn with_output_schema(mut self, output_schema: Value) -> Self {
        self.output_schema = Some(output_schema);
        self
    }

    /// Opt into strict structured calling. Stored only; not wired today.
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_spec_defaults_are_non_strict_without_output_schema() {
        let spec = ToolSpec::function("demo", "demo tool", json!({"type": "object"}));
        assert!(!spec.strict);
        assert_eq!(spec.output_schema, None);
    }

    #[test]
    fn builders_set_output_schema_and_strict() {
        let output = json!({"type": "object", "properties": {}});
        let spec = ToolSpec::function("demo", "demo tool", json!({"type": "object"}))
            .with_output_schema(output.clone())
            .with_strict(true);
        assert!(spec.strict);
        assert_eq!(spec.output_schema, Some(output));
    }

    #[test]
    fn internal_schema_omits_output_schema() {
        let spec = ToolSpec::function("demo", "demo tool", json!({"type": "object"}))
            .with_output_schema(json!({"type": "object"}));
        let internal = spec.to_internal_schema();
        assert!(internal.get("output_schema").is_none());
        assert_eq!(internal["name"], "demo");
    }
}
