#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentToolPolicy {
    All,
    AllowOnly(Vec<String>),
}

impl AgentToolPolicy {
    pub fn all() -> Self {
        Self::All
    }

    pub fn allow_only<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::AllowOnly(names.into_iter().map(Into::into).collect())
    }

    pub fn allows(&self, tool_name: &str) -> bool {
        match self {
            Self::All => true,
            Self::AllowOnly(names) => names.iter().any(|name| name == tool_name),
        }
    }
}
