import type { AgentNode, AgentRunStatus, AgentTreeNode, AgentTreeResponse, BackendRuntimeEvent, ThreadView } from "@/types";

/**
 * Flat, order-preserving representation of a single subagent. The synthetic
 * root agent (the active thread) is never stored as a record; it is rebuilt
 * from the live session status every time the forest is assembled.
 */
interface AgentRecord {
  threadId: string;
  parentThreadId: string | null;
  name: string;
  agentPath: string | null;
  status: AgentRunStatus;
  task: string;
  lastActivity: string | null;
  agentType: AgentNode["agentType"];
  seq: number;
}

function shortThread(threadId: string): string {
  const tail = threadId.split(/[_-]/).filter(Boolean).pop() ?? threadId;
  return tail.length > 8 ? `agent ${tail.slice(-6)}` : `agent ${tail}`;
}

function nextSeq(records: AgentRecord[]): number {
  return records.reduce((max, record) => Math.max(max, record.seq), -1) + 1;
}

function upsertSpawn(
  records: AgentRecord[],
  fields: { childThreadId: string; parentThreadId: string; taskName: string; messagePreview: string }
): AgentRecord[] {
  const name = fields.taskName.trim() || shortThread(fields.childThreadId);
  const existing = records.find((record) => record.threadId === fields.childThreadId);
  if (existing) {
    return records.map((record) =>
      record.threadId === fields.childThreadId
        ? { ...record, parentThreadId: fields.parentThreadId, name, task: fields.messagePreview }
        : record
    );
  }
  return [
    ...records,
    {
      threadId: fields.childThreadId,
      parentThreadId: fields.parentThreadId,
      name,
      agentPath: null,
      status: "running",
      task: fields.messagePreview,
      lastActivity: null,
      agentType: null,
      seq: nextSeq(records)
    }
  ];
}

function markClosed(
  records: AgentRecord[],
  fields: { closedThreadId: string; agentPath: string }
): AgentRecord[] {
  const existing = records.find((record) => record.threadId === fields.closedThreadId);
  if (!existing) {
    return [
      ...records,
      {
        threadId: fields.closedThreadId,
        parentThreadId: null,
        name: agentPathName(fields.agentPath) ?? shortThread(fields.closedThreadId),
        agentPath: fields.agentPath || null,
        status: "done",
        task: "",
        lastActivity: null,
        agentType: null,
        seq: nextSeq(records)
      }
    ];
  }
  return records.map((record) =>
    record.threadId === fields.closedThreadId
      ? { ...record, status: "done", agentPath: fields.agentPath || record.agentPath }
      : record
  );
}

function setActivity(
  records: AgentRecord[],
  fields: { recipientThreadId: string; content: string }
): AgentRecord[] {
  if (!records.some((record) => record.threadId === fields.recipientThreadId)) {
    return records;
  }
  return records.map((record) =>
    record.threadId === fields.recipientThreadId
      ? { ...record, lastActivity: fields.content }
      : record
  );
}

function agentPathName(agentPath: string | null | undefined): string | null {
  if (!agentPath) {
    return null;
  }
  const tail = agentPath.split("/").filter(Boolean).pop();
  return tail ?? null;
}

function statusFromTreeStatus(status: AgentTreeNode["status"]): AgentRunStatus {
  switch (status) {
    case "running":
      return "running";
    case "done":
      return "done";
    case "failed":
      return "failed";
    case "idle":
      return "idle";
    default:
      return "idle";
  }
}

function nodeName(node: AgentTreeNode, isRoot: boolean): string {
  if (isRoot) {
    return "Root agent";
  }
  return node.agent_nickname?.trim() || agentPathName(node.agent_path) || shortThread(node.thread_id ?? node.agent_path);
}

/** Fold a single backend runtime event into the subagent record list. */
export function applyAgentEvent(records: AgentRecord[], event: BackendRuntimeEvent): AgentRecord[] {
  switch (event.kind.type) {
    case "subagent_spawned":
      return upsertSpawn(records, {
        childThreadId: event.kind.child_thread_id,
        parentThreadId: event.kind.parent_thread_id,
        taskName: event.kind.task_name,
        messagePreview: event.kind.message_preview
      });
    case "subagent_closed":
      return markClosed(records, {
        closedThreadId: event.kind.closed_thread_id,
        agentPath: event.kind.agent_path
      });
    case "inter_agent_message_sent":
      return setActivity(records, {
        recipientThreadId: event.kind.recipient_thread_id,
        content: event.kind.content_preview
      });
    default:
      return records;
  }
}

/** Build the subagent record list from a resumed thread's persisted history. */
export function agentRecordsFromThreadView(thread: ThreadView): AgentRecord[] {
  let records: AgentRecord[] = [];
  for (const turn of thread.turns) {
    for (const item of turn.items) {
      switch (item.type) {
        case "subagent_spawn":
          records = upsertSpawn(records, {
            childThreadId: item.child_thread_id,
            parentThreadId: item.parent_thread_id,
            taskName: item.task_name,
            messagePreview: item.message_preview
          });
          break;
        case "subagent_close":
          records = markClosed(records, {
            closedThreadId: item.closed_thread_id,
            agentPath: item.agent_path
          });
          break;
        case "inter_agent_message":
          records = setActivity(records, {
            recipientThreadId: item.recipient_thread_id,
            content: item.content_preview
          });
          break;
        default:
          break;
      }
    }
  }
  return records;
}

/** Flatten an assembled forest back into subagent records (root excluded). */
export function flattenAgentForest(agents: AgentNode[]): AgentRecord[] {
  const records: AgentRecord[] = [];
  let seq = 0;
  const walk = (nodes: AgentNode[]) => {
    for (const node of nodes) {
      if (!node.isRoot) {
        records.push({
          threadId: node.threadId,
          parentThreadId: node.parentThreadId,
          name: node.name,
          agentPath: node.agentPath,
          status: node.status,
          task: node.task,
          lastActivity: node.lastActivity,
          agentType: node.agentType ?? null,
          seq: seq++
        });
      }
      walk(node.children);
    }
  };
  walk(agents);
  return records;
}

/**
 * Assemble the agent forest: a single root node (the active thread) with every
 * subagent nested under its parent. Subagents whose parent is unknown attach to
 * the root so nothing is dropped.
 */
export function buildAgentForest(
  rootThreadId: string | null,
  rootStatus: AgentRunStatus,
  records: AgentRecord[]
): AgentNode[] {
  if (!rootThreadId) {
    return [];
  }

  const root: AgentNode = {
    threadId: rootThreadId,
    parentThreadId: null,
    name: "Root agent",
    agentPath: null,
    status: rootStatus,
    task: "",
    lastActivity: null,
    agentType: null,
    isRoot: true,
    children: []
  };

  const ordered = [...records].sort((left, right) => left.seq - right.seq);
  const nodeByThread = new Map<string, AgentNode>([[rootThreadId, root]]);
  for (const record of ordered) {
    nodeByThread.set(record.threadId, {
      threadId: record.threadId,
      parentThreadId: record.parentThreadId,
      name: record.name,
      agentPath: record.agentPath,
      status: record.status,
      task: record.task,
      lastActivity: record.lastActivity,
      agentType: record.agentType ?? null,
      isRoot: false,
      children: []
    });
  }
  for (const record of ordered) {
    const node = nodeByThread.get(record.threadId);
    if (!node) {
      continue;
    }
    const parent =
      (record.parentThreadId ? nodeByThread.get(record.parentThreadId) : undefined) ?? root;
    parent.children.push(node);
  }

  return [root];
}

/** Count agents that are still live (spawning or running) across the forest. */
export function countLiveAgents(agents: AgentNode[]): number {
  let live = 0;
  const walk = (nodes: AgentNode[]) => {
    for (const node of nodes) {
      if (!node.isRoot && (node.status === "running" || node.status === "spawning")) {
        live += 1;
      }
      walk(node.children);
    }
  };
  walk(agents);
  return live;
}

/** Count every agent node in the forest, including the synthetic root. */
export function countAgents(agents: AgentNode[]): number {
  let total = 0;
  const walk = (nodes: AgentNode[]) => {
    for (const node of nodes) {
      total += 1;
      walk(node.children);
    }
  };
  walk(agents);
  return total;
}

/** Convert the app-server root-tree projection into the desktop AgentNode shape. */
export function agentForestFromTreeResponse(response: AgentTreeResponse): AgentNode[] {
  const convert = (node: AgentTreeNode, parentThreadId: string | null, isRoot: boolean): AgentNode => {
    const threadId = node.thread_id ?? node.agent_path;
    return {
      threadId,
      parentThreadId: node.parent_thread_id ?? parentThreadId,
      name: nodeName(node, isRoot),
      agentPath: node.agent_path || null,
      status: statusFromTreeStatus(node.status),
      task: node.last_task_message ?? "",
      lastActivity: node.last_activity ?? null,
      agentType: node.agent_type ?? null,
      role: node.agent_role ?? null,
      nickname: node.agent_nickname ?? null,
      isRoot,
      children: (node.children ?? []).map((child) => convert(child, threadId, false))
    };
  };
  return [convert(response.root, null, true)];
}
