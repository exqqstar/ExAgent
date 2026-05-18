use std::any::type_name;

use exagent::entrypoints::{api, cli, cli_adapter};
use exagent::model::{llm, types};
use exagent::runtime::{agent, exec_session, policy, thread_runtime};
use exagent::state::{events, session, transcript};
use exagent::tools::registry;

use exagent::{
    agent as compat_agent, events as compat_events, exec_session as compat_exec_session,
};
use exagent::{llm as compat_llm, policy as compat_policy, registry as compat_registry};
use exagent::{session as compat_session, transcript as compat_transcript, types as compat_types};

#[test]
fn canonical_and_compatibility_module_paths_compile() {
    let names = [
        type_name::<api::ThreadStartRequest>(),
        type_name::<cli::CliCommand>(),
        type_name::<cli_adapter::CliExecutionOutput>(),
        type_name::<llm::OpenAiCompatibleLlm>(),
        type_name::<types::AssistantTurn>(),
        type_name::<agent::Agent>(),
        type_name::<exec_session::ExecSessionManager>(),
        type_name::<policy::PolicyManager>(),
        type_name::<thread_runtime::ThreadRuntimeStatus>(),
        type_name::<events::RuntimeEventKind>(),
        type_name::<session::SessionSnapshot>(),
        type_name::<transcript::SessionPaths>(),
        type_name::<registry::ToolRegistry>(),
        type_name::<compat_agent::Agent>(),
        type_name::<compat_exec_session::ExecSessionManager>(),
        type_name::<compat_policy::PolicyManager>(),
        type_name::<compat_events::RuntimeEventKind>(),
        type_name::<compat_session::SessionSnapshot>(),
        type_name::<compat_transcript::SessionPaths>(),
        type_name::<compat_llm::OpenAiCompatibleLlm>(),
        type_name::<compat_types::AssistantTurn>(),
        type_name::<compat_registry::ToolRegistry>(),
    ];

    assert!(names.iter().all(|name| !name.is_empty()));
}
