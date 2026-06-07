use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;

use crate::app_server::override_policy::OverridePolicy;
use crate::app_server::protocol::{
    AgentTreeAgentStatus, AgentTreeNode, AgentTreeParams, AgentTreeResponse, TurnStatus,
};
use crate::app_server::services::AppServerServices;
use crate::app_server::thread_store::{read_thread_state_from_storage, StoredThreadState};
use crate::app_server::AppServerError;
use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::runtime::agent_profile::AgentType;
use crate::state::spawn_edges::{SpawnEdgeStatus, ThreadSpawnEdge, ThreadSpawnEdgeStore};
use crate::types::ThreadId;

const ROOT_AGENT_PATH: &str = "root";

#[derive(Debug, Clone)]
struct AgentRecord {
    thread_id: ThreadId,
    parent_thread_id: Option<ThreadId>,
    root_thread_id: ThreadId,
    depth: u32,
    agent_path: String,
    status: AgentTreeAgentStatus,
    agent_type: Option<AgentType>,
    agent_role: Option<String>,
    agent_nickname: Option<String>,
    last_task_message: Option<String>,
    last_activity: Option<String>,
}

pub(in crate::app_server) fn agent_tree(
    services: &AppServerServices,
    params: AgentTreeParams,
) -> Result<AgentTreeResponse> {
    let requested_workspace_root = params.workspace_root.clone();
    let requested_workspace_root = requested_workspace_root.is_some();
    let config = OverridePolicy::merge_thread_read(&services.base_config, params.workspace_root)?;
    let workspace_root = services
        .runtime_loader
        .resolve_loaded_runtime(
            &params.thread_id,
            requested_workspace_root,
            &config.workspace_root,
        )?
        .map(|loaded| loaded.workspace_root)
        .unwrap_or(config.workspace_root);

    let requested = read_thread_state(services, &workspace_root, &params.thread_id)?
        .ok_or_else(|| AppServerError::ThreadNotFound(params.thread_id.clone()))?;
    let root_thread_id = requested
        .snapshot
        .lineage
        .as_ref()
        .map(|lineage| lineage.root_thread_id.clone())
        .unwrap_or_else(|| params.thread_id.clone());
    let root = if root_thread_id == params.thread_id {
        requested
    } else {
        read_thread_state(services, &workspace_root, &root_thread_id)?
            .ok_or_else(|| AppServerError::ThreadNotFound(root_thread_id.clone()))?
    };

    let edge_store = ThreadSpawnEdgeStore::for_workspace(&workspace_root);
    let mut edges = edge_store.list_by_root_blocking(&root_thread_id, None)?;
    edges.sort_by(|left, right| {
        left.agent_path.cmp(&right.agent_path).then_with(|| {
            left.child_thread_id
                .as_str()
                .cmp(right.child_thread_id.as_str())
        })
    });

    let mut task_by_thread = HashMap::new();
    let mut activity_by_thread = HashMap::new();
    collect_agent_event_details(&root.events, &mut task_by_thread, &mut activity_by_thread);

    let mut child_states = HashMap::new();
    for edge in &edges {
        if let Some(state) = read_thread_state(services, &workspace_root, &edge.child_thread_id)? {
            collect_agent_event_details(
                &state.events,
                &mut task_by_thread,
                &mut activity_by_thread,
            );
            child_states.insert(edge.child_thread_id.clone(), state);
        }
    }

    let mut records = HashMap::new();
    records.insert(
        root_thread_id.clone(),
        AgentRecord {
            thread_id: root_thread_id.clone(),
            parent_thread_id: None,
            root_thread_id: root_thread_id.clone(),
            depth: 0,
            agent_path: ROOT_AGENT_PATH.to_string(),
            status: if services
                .runtime_loader
                .active_turn_state(&root_thread_id)
                .is_some()
            {
                AgentTreeAgentStatus::Running
            } else {
                AgentTreeAgentStatus::Idle
            },
            agent_type: root
                .snapshot
                .lineage
                .as_ref()
                .and_then(|lineage| lineage.agent_type),
            agent_role: root
                .snapshot
                .lineage
                .as_ref()
                .and_then(|lineage| lineage.agent_role.clone()),
            agent_nickname: root
                .snapshot
                .lineage
                .as_ref()
                .and_then(|lineage| lineage.agent_nickname.clone()),
            last_task_message: None,
            last_activity: activity_by_thread.get(&root_thread_id).cloned(),
        },
    );

    for edge in edges {
        let state = child_states.get(&edge.child_thread_id);
        let snapshot = state.map(|state| &state.snapshot);
        let lineage = snapshot.and_then(|snapshot| snapshot.lineage.as_ref());
        records.insert(
            edge.child_thread_id.clone(),
            AgentRecord {
                thread_id: edge.child_thread_id.clone(),
                parent_thread_id: Some(edge.parent_thread_id.clone()),
                root_thread_id: edge.root_thread_id.clone(),
                depth: lineage
                    .map(|lineage| lineage.depth)
                    .unwrap_or_else(|| depth_from_path(&edge.agent_path)),
                agent_path: lineage
                    .map(|lineage| lineage.agent_path.clone())
                    .unwrap_or_else(|| edge.agent_path.clone()),
                status: status_from_edge(
                    services,
                    &edge.child_thread_id,
                    &edge,
                    state.map(|state| state.events.as_slice()),
                ),
                agent_type: lineage.and_then(|lineage| lineage.agent_type),
                agent_role: lineage.and_then(|lineage| lineage.agent_role.clone()),
                agent_nickname: lineage.and_then(|lineage| lineage.agent_nickname.clone()),
                last_task_message: task_by_thread.get(&edge.child_thread_id).cloned(),
                last_activity: activity_by_thread.get(&edge.child_thread_id).cloned(),
            },
        );
    }

    let root_node = build_node(&root_thread_id, &records);
    Ok(AgentTreeResponse { root: root_node })
}

fn read_thread_state(
    services: &AppServerServices,
    workspace_root: &Path,
    thread_id: &ThreadId,
) -> Result<Option<StoredThreadState>> {
    if let Some(loaded) =
        services
            .runtime_loader
            .resolve_loaded_runtime(thread_id, false, workspace_root)?
    {
        let live_view = loaded.runtime.live_view();
        return Ok(Some(StoredThreadState {
            snapshot: live_view.snapshot,
            response_items: read_thread_state_from_storage(&loaded.workspace_root, thread_id)?
                .map(|stored| stored.response_items)
                .unwrap_or_default(),
            events: live_view.events,
        }));
    }
    read_thread_state_from_storage(workspace_root, thread_id)
}

fn collect_agent_event_details(
    events: &[RuntimeEvent],
    task_by_thread: &mut HashMap<ThreadId, String>,
    activity_by_thread: &mut HashMap<ThreadId, String>,
) {
    for event in events {
        match &event.kind {
            RuntimeEventKind::SubagentSpawned {
                child_thread_id,
                message_preview,
                ..
            } => {
                task_by_thread.insert(child_thread_id.clone(), message_preview.clone());
            }
            RuntimeEventKind::InterAgentMessageSent {
                author_thread_id,
                recipient_thread_id,
                content_preview,
                ..
            } => {
                activity_by_thread.insert(author_thread_id.clone(), content_preview.clone());
                activity_by_thread.insert(recipient_thread_id.clone(), content_preview.clone());
            }
            _ => {}
        }
    }
}

fn build_node(thread_id: &ThreadId, records: &HashMap<ThreadId, AgentRecord>) -> AgentTreeNode {
    let record = records
        .get(thread_id)
        .expect("agent tree records must contain requested node");
    let mut child_ids = records
        .values()
        .filter(|candidate| candidate.parent_thread_id.as_ref() == Some(thread_id))
        .map(|candidate| candidate.thread_id.clone())
        .collect::<Vec<_>>();
    child_ids.sort_by(|left, right| {
        let left_path = records
            .get(left)
            .map(|record| record.agent_path.as_str())
            .unwrap_or_default();
        let right_path = records
            .get(right)
            .map(|record| record.agent_path.as_str())
            .unwrap_or_default();
        left_path
            .cmp(right_path)
            .then_with(|| left.as_str().cmp(right.as_str()))
    });

    AgentTreeNode {
        thread_id: Some(record.thread_id.clone()),
        parent_thread_id: record.parent_thread_id.clone(),
        root_thread_id: record.root_thread_id.clone(),
        depth: record.depth,
        agent_path: record.agent_path.clone(),
        status: record.status,
        agent_type: record.agent_type,
        agent_role: record.agent_role.clone(),
        agent_nickname: record.agent_nickname.clone(),
        last_task_message: record.last_task_message.clone(),
        last_activity: record.last_activity.clone(),
        children: child_ids
            .iter()
            .map(|child_id| build_node(child_id, records))
            .collect(),
    }
}

fn status_from_edge(
    services: &AppServerServices,
    child_thread_id: &ThreadId,
    edge: &ThreadSpawnEdge,
    events: Option<&[RuntimeEvent]>,
) -> AgentTreeAgentStatus {
    match edge.status {
        SpawnEdgeStatus::Closed => AgentTreeAgentStatus::Done,
        SpawnEdgeStatus::Open
            if services
                .runtime_loader
                .active_turn_state(child_thread_id)
                .is_some() =>
        {
            AgentTreeAgentStatus::Running
        }
        SpawnEdgeStatus::Open
            if events
                .and_then(crate::app_server::thread_projection::latest_turn_state)
                .is_some_and(|state| state.status == TurnStatus::Failed) =>
        {
            AgentTreeAgentStatus::Failed
        }
        SpawnEdgeStatus::Open => AgentTreeAgentStatus::Idle,
    }
}

fn depth_from_path(agent_path: &str) -> u32 {
    agent_path
        .split('/')
        .filter(|part| !part.is_empty())
        .count()
        .saturating_sub(1) as u32
}
