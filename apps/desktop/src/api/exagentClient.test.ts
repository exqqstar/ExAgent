import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { compactThread, importImageFiles, pickImageFiles, scanSkillCatalog, setThreadGoal } from "@/api/exagentClient";

const tauriMocks = vi.hoisted(() => ({
  invoke: vi.fn()
}));

const dialogMocks = vi.hoisted(() => ({
  open: vi.fn()
}));

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
      clearTokenBudget: true
    });

    expect(tauriMocks.invoke).toHaveBeenCalledWith("thread_goal_set", {
      projectId: "project-exagent",
      threadId: "thread-goal",
      objective: null,
      status: null,
      tokenBudget: null,
      clearTokenBudget: true
    });
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

  it("returns an empty compaction response in browser preview", async () => {
    delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;

    await expect(compactThread("project-exagent", "thread-root")).resolves.toEqual({
      thread_id: "thread-root",
      latest_compaction: null
    });
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
