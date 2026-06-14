import { afterEach, describe, expect, it, vi } from "vitest";
import { memoryForget, memoryPromote, memorySave, memoryUpdate } from "@/lib/api/memory";

const tauriMocks = vi.hoisted(() => ({
  invoke: vi.fn()
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: tauriMocks.invoke
}));

describe("memory api", () => {
  afterEach(() => {
    delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    tauriMocks.invoke.mockReset();
  });

  it("does not send write commands outside the Tauri runtime", async () => {
    delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    tauriMocks.invoke.mockResolvedValue({});

    await expect(
      memorySave("project-1", "project", {
        kind: "fact",
        title: "Preview write",
        content: "Should not invoke desktop commands in browser preview."
      })
    ).rejects.toThrow("Memory writes require the desktop runtime");
    await expect(memoryUpdate("project-1", "entry-1", "pin", "project")).rejects.toThrow(
      "Memory writes require the desktop runtime"
    );
    await expect(memoryForget("project-1", "entry-1", "project")).rejects.toThrow(
      "Memory writes require the desktop runtime"
    );
    await expect(memoryPromote("project-1", "entry-1", "project", false)).rejects.toThrow(
      "Memory writes require the desktop runtime"
    );

    expect(tauriMocks.invoke).not.toHaveBeenCalled();
  });
});
