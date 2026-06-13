import { describe, expect, it } from "vitest";
import {
  agentRecordsFromThreadView,
  applyAgentEvent,
  buildAgentForest,
  countLiveAgents,
  flattenAgentForest,
  agentForestFromTreeResponse
} from "@/lib/agentTree";
import type { AgentTreeResponse, BackendRuntimeEvent, BackendRuntimeEventKind, ThreadView } from "@/types";

const ROOT = "thread_root";

function event(kind: BackendRuntimeEventKind, threadId = ROOT): BackendRuntimeEvent {
  return { event_id: `evt_${Math.random().toString(36).slice(2)}`, thread_id: threadId, kind };
}

function spawn(child: string, parent: string, taskName: string, preview = ""): BackendRuntimeEvent {
  return event({
    type: "subagent_spawned",
    invocation_id: `inv_${child}`,
    tool_call_id: `call_${child}`,
    parent_thread_id: parent,
    child_thread_id: child,
    task_name: taskName,
    message_preview: preview
  });
}

function foldEvents(events: BackendRuntimeEvent[]) {
  return events.reduce((records, current) => applyAgentEvent(records, current), [] as ReturnType<typeof flattenAgentForest>);
}

describe("buildAgentForest", () => {
  it("returns a single root when no subagents exist", () => {
    const forest = buildAgentForest(ROOT, "idle", []);
    expect(forest).toHaveLength(1);
    expect(forest[0].isRoot).toBe(true);
    expect(forest[0].children).toHaveLength(0);
    expect(countLiveAgents(forest)).toBe(0);
  });

  it("returns nothing when there is no active thread", () => {
    expect(buildAgentForest(null, "idle", [])).toEqual([]);
  });

  it("nests a spawned subagent under the root", () => {
    const records = foldEvents([spawn("thread_a", ROOT, "researcher", "find the bug")]);
    const forest = buildAgentForest(ROOT, "running", records);
    const [root] = forest;
    expect(root.children).toHaveLength(1);
    expect(root.children[0].name).toBe("researcher");
    expect(root.children[0].status).toBe("running");
    expect(root.children[0].task).toBe("find the bug");
    expect(countLiveAgents(forest)).toBe(1);
  });

  it("builds a recursive tree from grandchild spawns", () => {
    const records = foldEvents([
      spawn("thread_a", ROOT, "researcher"),
      spawn("thread_b", "thread_a", "scraper")
    ]);
    const [root] = buildAgentForest(ROOT, "running", records);
    expect(root.children).toHaveLength(1);
    expect(root.children[0].children).toHaveLength(1);
    expect(root.children[0].children[0].name).toBe("scraper");
    expect(root.children[0].children[0].parentThreadId).toBe("thread_a");
  });

  it("attaches orphans whose parent is unknown to the root", () => {
    const records = foldEvents([spawn("thread_x", "thread_missing", "stray")]);
    const [root] = buildAgentForest(ROOT, "running", records);
    expect(root.children).toHaveLength(1);
    expect(root.children[0].name).toBe("stray");
  });
});

describe("applyAgentEvent", () => {
  it("marks a closed subagent done but keeps it in the tree", () => {
    const records = foldEvents([
      spawn("thread_a", ROOT, "researcher"),
      event({
        type: "subagent_closed",
        invocation_id: "inv_close",
        tool_call_id: "call_close",
        parent_thread_id: ROOT,
        closed_thread_id: "thread_a",
        agent_path: "root/researcher"
      })
    ]);
    const [root] = buildAgentForest(ROOT, "running", records);
    expect(root.children).toHaveLength(1);
    expect(root.children[0].status).toBe("done");
    expect(countLiveAgents([root])).toBe(0);
  });

  it("records inter-agent message previews as last activity on the recipient", () => {
    const records = foldEvents([
      spawn("thread_a", ROOT, "researcher"),
      event({
        type: "inter_agent_message_sent",
        invocation_id: "inv_msg",
        tool_call_id: "call_msg",
        author_thread_id: ROOT,
        recipient_thread_id: "thread_a",
        author_path: "root",
        recipient_path: "root/researcher",
        content_preview: "also check the logs",
        followup: true
      })
    ]);
    const [root] = buildAgentForest(ROOT, "running", records);
    expect(root.children[0].lastActivity).toBe("also check the logs");
  });

  it("is idempotent for a repeated spawn event", () => {
    const single = foldEvents([spawn("thread_a", ROOT, "researcher")]);
    const repeated = foldEvents([spawn("thread_a", ROOT, "researcher"), spawn("thread_a", ROOT, "researcher")]);
    expect(repeated).toHaveLength(single.length);
  });
});

describe("flatten / build round trip", () => {
  it("preserves the tree across a flatten then rebuild", () => {
    const records = foldEvents([
      spawn("thread_a", ROOT, "researcher"),
      spawn("thread_b", "thread_a", "scraper")
    ]);
    const forest = buildAgentForest(ROOT, "running", records);
    const rebuilt = buildAgentForest(ROOT, "running", flattenAgentForest(forest));
    expect(rebuilt[0].children[0].children[0].name).toBe("scraper");
  });
});

describe("agentRecordsFromThreadView", () => {
  it("reconstructs subagents from persisted thread history", () => {
    const thread: ThreadView = {
      id: ROOT,
      status: "idle",
      goal_mode: "standard",
      active_turn: null,
      turns: [
        {
          id: "turn_1",
          status: "completed",
          items: [
            {
              type: "subagent_spawn",
              invocation_id: "inv_a",
              tool_call_id: "call_a",
              parent_thread_id: ROOT,
              child_thread_id: "thread_a",
              task_name: "researcher",
              message_preview: "investigate"
            }
          ]
        }
      ]
    };
    const [root] = buildAgentForest(ROOT, "idle", agentRecordsFromThreadView(thread));
    expect(root.children).toHaveLength(1);
    expect(root.children[0].name).toBe("researcher");
  });
});

describe("agentForestFromTreeResponse", () => {
  it("maps app-server agent tree responses into UI agent nodes", () => {
    const response: AgentTreeResponse = {
      root: {
        thread_id: "thread_root",
        root_thread_id: "thread_root",
        depth: 0,
        agent_path: "root",
        status: "idle",
        children: [
          {
            thread_id: "thread_research",
            parent_thread_id: "thread_root",
            root_thread_id: "thread_root",
            depth: 1,
            agent_path: "root/researcher",
            status: "running",
            agent_type: "explorer",
            agent_nickname: "Rhea",
            agent_role: "research role",
            last_task_message: "map the inspector state",
            last_activity: "also check activeSessionId consumers",
            current_tool: "search_files",
            tokens_used: 12345,
            children: [
              {
                thread_id: "thread_scraper",
                parent_thread_id: "thread_research",
                root_thread_id: "thread_root",
                depth: 2,
                agent_path: "root/researcher/scraper",
                status: "running",
                last_task_message: "read protocol.rs",
                children: []
              }
            ]
          }
        ]
      }
    };

    const [root] = agentForestFromTreeResponse(response);

    expect(root.threadId).toBe("thread_root");
    expect(root.children[0].name).toBe("Rhea");
    expect(root.children[0].agentType).toBe("explorer");
    expect(root.children[0].role).toBe("research role");
    expect(root.children[0].task).toBe("map the inspector state");
    expect(root.children[0].lastActivity).toBe("also check activeSessionId consumers");
    expect(root.children[0].currentTool).toBe("search_files");
    expect(root.children[0].tokensUsed).toBe(12345);
    expect(root.children[0].children[0].name).toBe("scraper");
  });

  it("counts waiting-approval child agents as live", () => {
    const response: AgentTreeResponse = {
      root: {
        thread_id: "thread_root",
        root_thread_id: "thread_root",
        depth: 0,
        agent_path: "root",
        status: "idle",
        children: [
          {
            thread_id: "thread_reviewer",
            parent_thread_id: "thread_root",
            root_thread_id: "thread_root",
            depth: 1,
            agent_path: "root/reviewer",
            status: "waiting_approval",
            last_task_message: "approve the command",
            children: []
          }
        ]
      }
    };

    const forest = agentForestFromTreeResponse(response);

    expect(forest[0].children[0].status).toBe("waiting_approval");
    expect(countLiveAgents(forest)).toBe(1);
  });
});
