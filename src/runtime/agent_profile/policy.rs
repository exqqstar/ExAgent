#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceToolCapability {
    ReadOnly,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollaborationToolCapability {
    Basic,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentToolPolicy {
    pub workspace: WorkspaceToolCapability,
    pub collaboration: CollaborationToolCapability,
}

impl AgentToolPolicy {
    pub fn all() -> Self {
        Self {
            workspace: WorkspaceToolCapability::Full,
            collaboration: CollaborationToolCapability::Full,
        }
    }

    pub fn read_only_basic_collaboration() -> Self {
        Self {
            workspace: WorkspaceToolCapability::ReadOnly,
            collaboration: CollaborationToolCapability::Basic,
        }
    }

    pub fn allows(&self, tool_name: &str) -> bool {
        match tool_name {
            "read_file" | "search_files" | "web_search" => true,
            "write_file" | "run_command" | "apply_patch" | "exec_command" | "write_stdin" => {
                self.workspace == WorkspaceToolCapability::Full
            }
            "list_agents" | "send_message" | "wait_agent" => true,
            "followup_task" | "spawn_agent" | "close_agent" => {
                self.collaboration == CollaborationToolCapability::Full
            }
            _ => self.workspace == WorkspaceToolCapability::Full,
        }
    }
}
