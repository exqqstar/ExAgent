import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  compactThread,
  cancelWorkflow,
  getThreadGoal,
  importImageFiles,
  listApprovals,
  pickImageFiles,
  readWorkflow,
  replayAllEvents,
  scanSkillCatalog,
  setThreadGoal,
  submitApprovalDecision
} from "@/api/exagentClient";
import type { BackendRuntimeEvent } from "@/types";

const tauriMocks = vi.hoisted(() => ({
  invoke: vi.fn()
}));

const dialogMocks = vi.hoisted(() => ({
  open: vi.fn()
}));

function runtimeEvent(eventId: string): BackendRuntimeEvent {
  return {
    event_id: eventId,
    thread_id: "thread-root",
    turn_id: "turn-1",
    kind: { type: "turn_started" }
  };
}

vi.mock("@tauri-apps/api/core", () => ({
  invoke: tauriMocks.invoke
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: dialogMocks.open
}));

describe("exagentClient", () => {
  beforeEach(() => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      value: {},
      configurable: true
    });
    tauriMocks.invoke.mockResolvedValue({ goal: null });
    dialogMocks.open.mockReset();
  });

  afterEach(() => {
    delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    tauriMocks.invoke.mockReset();
    dialogMocks.open.mockReset();
  });

  it("sends an explicit clear-token-budget flag to the desktop command", async () => {
    await setThreadGoal("project-exagent", "thread-goal", {
      tokenBudget: null,
      clearTokenBudget: true,
      mode: "reviewed"
    });

    expect(tauriMocks.invoke).toHaveBeenCalledWith("thread_goal_set", {
      projectId: "project-exagent",
      threadId: "thread-goal",
      objective: null,
      status: null,
      tokenBudget: null,
      clearTokenBudget: true,
      mode: "reviewed"
    });
  });

  it("returns standard goal mode in browser preview", async () => {
    delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;

    await expect(getThreadGoal("project-exagent", "thread-goal")).resolves.toEqual({
      goal: null,
      mode: "standard"
    });
    expect(tauriMocks.invoke).not.toHaveBeenCalled();
  });

  it("compacts a thread through the desktop command", async () => {
    tauriMocks.invoke.mockResolvedValue({
      thread_id: "thread-root",
      latest_compaction: { summary: "manual compact summary" }
    });

    const response = await compactThread("project-exagent", "thread-root");

    expect(tauriMocks.invoke).toHaveBeenCalledWith("thread_compact", {
      projectId: "project-exagent",
      threadId: "thread-root"
    });
    expect(response).toEqual({
      thread_id: "thread-root",
      latest_compaction: { summary: "manual compact summary" }
    });
  });

  it("reads and cancels workflow runs through desktop commands", async () => {
    const run = {
      run_id: "workflow_run_thread-root",
      thread_id: "thread-root",
      template_id: "deep-research",
      preset_id: "quick",
      label: "Deep research: web search",
      status: "running",
      phases: [],
      artifacts: [],
      stats: {
        agent_calls: 1,
        failed_agent_calls: 0,
        skipped_agent_calls: 0,
        total_artifacts: 0,
        elapsed_ms: 12,
        template_stats: {}
      },
      created_at_ms: 1,
      updated_at_ms: 2
    };
    tauriMocks.invoke.mockResolvedValue({ run });

    await expect(readWorkflow("project-exagent", "workflow_run_thread-root")).resolves.toEqual({ run });
    await expect(cancelWorkflow("project-exagent", "workflow_run_thread-root")).resolves.toEqual({ run });

    expect(tauriMocks.invoke).toHaveBeenNthCalledWith(1, "workflow_read", {
      projectId: "project-exagent",
      runId: "workflow_run_thread-root"
    });
    expect(tauriMocks.invoke).toHaveBeenNthCalledWith(2, "workflow_cancel", {
      projectId: "project-exagent",
      runId: "workflow_run_thread-root"
    });
  });

  it("replays runtime events from a single snapshot without chasing newly appended events", async () => {
    const firstEvent = runtimeEvent("evt_1");
    const secondEvent = runtimeEvent("evt_2");

    tauriMocks.invoke.mockImplementation(async (_command, args) => {
      if (args?.afterEventId === null) {
        return { thread_id: "thread-root", events: [firstEvent, secondEvent] };
      }
      throw new Error(`unexpected cursor ${String(args?.afterEventId)}`);
    });

    const events = await replayAllEvents("project-exagent", "thread-root");

    expect(events).toEqual([firstEvent, secondEvent]);
    expect(tauriMocks.invoke).toHaveBeenCalledTimes(1);
    expect(tauriMocks.invoke).toHaveBeenCalledWith("events_replay", {
      projectId: "project-exagent",
      threadId: "thread-root",
      afterEventId: null,
      includeSnapshot: true
    });
  });

  it("returns an empty compaction response in browser preview", async () => {
    delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;

    await expect(compactThread("project-exagent", "thread-root")).resolves.toEqual({
      thread_id: "thread-root",
      latest_compaction: null
    });
    expect(tauriMocks.invoke).not.toHaveBeenCalled();
  });

  it("removes browser-preview approvals through the approval decision path", async () => {
    delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;

    const before = await listApprovals("project-exagent");
    expect(before.approvals.map((item) => item.approval_id)).toContain("approval-preview-command");

    await submitApprovalDecision(
      "project-exagent",
      "session-desktop",
      undefined,
      "approval-preview-command",
      "approved",
      "desktop approved"
    );

    const after = await listApprovals("project-exagent");
    expect(after.approvals.map((item) => item.approval_id)).not.toContain("approval-preview-command");
    expect(tauriMocks.invoke).not.toHaveBeenCalled();
  });

  it("scans the skill catalog through the desktop command", async () => {
    tauriMocks.invoke.mockResolvedValue({
      sources: [],
      skills: [],
      warnings: []
    });

    await scanSkillCatalog("/workspace/project");

    expect(tauriMocks.invoke).toHaveBeenCalledWith("skill_catalog_scan", {
      workspaceRoot: "/workspace/project"
    });
  });

  it("imports picked image files into the desktop attachment cache", async () => {
    dialogMocks.open.mockResolvedValue(["/Users/me/Desktop/screen.png"]);
    tauriMocks.invoke.mockResolvedValue([
      "/Users/me/Library/Caches/io.github.exqqstar.exagent/attachments/hash/screen.png"
    ]);

    const paths = await pickImageFiles();

    expect(dialogMocks.open).toHaveBeenCalledWith({
      directory: false,
      multiple: true,
      filters: [
        {
          name: "Images",
          extensions: ["png", "jpg", "jpeg", "webp", "gif"]
        }
      ]
    });
    expect(tauriMocks.invoke).toHaveBeenCalledWith("image_attachments_import", {
      paths: ["/Users/me/Desktop/screen.png"]
    });
    expect(paths).toEqual([
      "/Users/me/Library/Caches/io.github.exqqstar.exagent/attachments/hash/screen.png"
    ]);
  });

  it("imports browser image files into the desktop attachment cache", async () => {
    const file = new File([new Uint8Array([137, 80, 78, 71])], "pasted.png", {
      type: "image/png"
    });
    tauriMocks.invoke.mockResolvedValue([
      "/Users/me/Library/Caches/io.github.exqqstar.exagent/attachments/hash/pasted.png"
    ]);

    const paths = await importImageFiles([file]);

    expect(tauriMocks.invoke).toHaveBeenCalledWith("image_attachments_import_bytes", {
      items: [
        {
          fileName: "pasted.png",
          mimeType: "image/png",
          bytesBase64: "iVBORw=="
        }
      ]
    });
    expect(paths).toEqual([
      "/Users/me/Library/Caches/io.github.exqqstar.exagent/attachments/hash/pasted.png"
    ]);
  });

  it("rejects oversized browser image files before invoking the desktop command", async () => {
    const file = new File([new Uint8Array([137, 80, 78, 71])], "huge.png", {
      type: "image/png"
    });
    Object.defineProperty(file, "size", {
      value: 20 * 1024 * 1024 + 1,
      configurable: true
    });

    await expect(importImageFiles([file])).rejects.toThrow(
      "Could not import image `huge.png`: 20971521 bytes exceeds the 20971520 byte limit"
    );
    expect(tauriMocks.invoke).not.toHaveBeenCalled();
  });

  it("rejects empty browser image files before invoking the desktop command", async () => {
    const file = new File([], "empty.png", {
      type: "image/png"
    });

    await expect(importImageFiles([file])).rejects.toThrow(
      "Could not import image `empty.png`: file is empty"
    );
    expect(tauriMocks.invoke).not.toHaveBeenCalled();
  });

  it("rejects too many browser image files before invoking the desktop command", async () => {
    const files = Array.from(
      { length: 9 },
      (_, index) =>
        new File([new Uint8Array([137, 80, 78, 71])], `image-${index}.png`, {
          type: "image/png"
        })
    );

    await expect(importImageFiles(files)).rejects.toThrow(
      "Could not import images: 9 files exceeds the 8 file limit"
    );
    expect(tauriMocks.invoke).not.toHaveBeenCalled();
  });

  it("rejects browser image batches over the byte limit before invoking the desktop command", async () => {
    const files = [
      new File([new Uint8Array([137, 80, 78, 71])], "first.png", {
        type: "image/png"
      }),
      new File([new Uint8Array([137, 80, 78, 71])], "second.png", {
        type: "image/png"
      })
    ];
    Object.defineProperty(files[0], "size", {
      value: 10 * 1024 * 1024,
      configurable: true
    });
    Object.defineProperty(files[1], "size", {
      value: 10 * 1024 * 1024 + 1,
      configurable: true
    });

    await expect(importImageFiles(files)).rejects.toThrow(
      "Could not import images: 20971521 total bytes exceeds the 20971520 byte batch limit"
    );
    expect(tauriMocks.invoke).not.toHaveBeenCalled();
  });

  it("returns a browser-preview skill catalog outside the desktop shell", async () => {
    delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;

    const scan = await scanSkillCatalog("/workspace/project");

    expect(scan.sources.map((source) => source.scope)).toContain("project");
    expect(scan.skills).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ name: "project-memory", scope: "project", effective_implicit: true }),
        expect.objectContaining({ name: "release-notes", scope: "global", effective_implicit: true }),
        expect.objectContaining({ name: "billing-audit", scope: "global", effective_implicit: false })
      ])
    );
  });
});
