import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { scanSkillCatalog, setThreadGoal } from "@/api/exagentClient";

const tauriMocks = vi.hoisted(() => ({
  invoke: vi.fn()
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: tauriMocks.invoke
}));

describe("exagentClient", () => {
  beforeEach(() => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      value: {},
      configurable: true
    });
    tauriMocks.invoke.mockResolvedValue({ goal: null });
  });

  afterEach(() => {
    delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    tauriMocks.invoke.mockReset();
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
