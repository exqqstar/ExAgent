use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,
    pub max_turns: usize,
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub command_timeout_secs: u64,
    pub max_output_bytes: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4.1".to_string()),
            max_turns: 12,
            workspace_root: cwd.clone(),
            cwd,
            command_timeout_secs: 30,
            max_output_bytes: 8 * 1024,
        }
    }
}
