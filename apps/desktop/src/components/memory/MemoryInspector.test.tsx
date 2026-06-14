import "@testing-library/jest-dom/vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryInspector } from "@/components/memory/MemoryInspector";
import * as memoryApi from "@/lib/api/memory";
import type { MemoryAuditResponse, MemoryListCandidatesResponse, MemorySearchResponse } from "@/lib/api/memory";

class ResizeObserverMock {
  observe() {}
  unobserve() {}
  disconnect() {}
}

vi.mock("@/lib/api/memory", () => ({
  memorySearch: vi.fn(),
  memorySave: vi.fn(),
  memoryUpdate: vi.fn(),
  memoryForget: vi.fn(),
  memoryAudit: vi.fn(),
  memoryListCandidates: vi.fn(),
  memoryPromote: vi.fn()
}));

const activeResponse: MemorySearchResponse = {
  hits: [
    {
      id: "entry-1",
      source: "entry",
      scope: "project",
      kind: "fact",
      title: "Use local approval checkpoints",
      body: "Approval rollback is backed by a checkpoint id.",
      files: [],
      concepts: ["desktop", "approval"],
      source_observation_ids: ["obs-entry-1"],
      confidence: 0.91,
      stale: false,
      quarantined: false,
      rank: 1,
      pinned: false,
      status: "active",
      use_count: 0
    }
  ]
};

const observationResponse: MemorySearchResponse = {
  hits: [
    {
      id: "observation-1",
      source: "observation",
      scope: "project",
      kind: "note",
      title: "Possible stale protocol note",
      body: "Observed in an older protocol draft.",
      files: ["src/protocol.rs"],
      concepts: ["protocol"],
      source_observation_ids: [],
      confidence: 0.35,
      stale: true,
      quarantined: true,
      rank: 1
    }
  ]
};

const candidatesResponse: MemoryListCandidatesResponse = {
  candidates: [
    {
      id: "candidate-1",
      scope: "project",
      kind: "preference",
      title: "Prefer compact inspector sections",
      body: "Use dense sections instead of table-heavy memory UI.",
      files: ["apps/desktop/src/components/Inspector.tsx"],
      concepts: ["desktop", "memory"],
      confidence: 0.72,
      pinned: false,
      status: "candidate",
      stale: false,
      quarantined: false,
      source_observation_ids: ["obs-candidate-1"],
      created_at_ms: 1_700_000_000_000,
      updated_at_ms: 1_700_000_000_000
    }
  ]
};

const auditResponse: MemoryAuditResponse = {
  events: []
};

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", ResizeObserverMock);
  vi.mocked(memoryApi.memorySearch).mockImplementation((_projectId, _scope, _query, includeObservations) =>
    Promise.resolve(includeObservations ? observationResponse : activeResponse)
  );
  vi.mocked(memoryApi.memoryListCandidates).mockResolvedValue(candidatesResponse);
  vi.mocked(memoryApi.memoryAudit).mockResolvedValue(auditResponse);
  vi.mocked(memoryApi.memoryPromote).mockResolvedValue({ entry: candidatesResponse.candidates[0] });
  vi.mocked(memoryApi.memorySave).mockResolvedValue({ entry: candidatesResponse.candidates[0] });
  vi.mocked(memoryApi.memoryUpdate).mockResolvedValue({ entry: candidatesResponse.candidates[0] });
  vi.mocked(memoryApi.memoryForget).mockResolvedValue({ forgotten: true });
});

describe("MemoryInspector", () => {
  it("shows observations as low-trust records distinct from active entries", async () => {
    render(<MemoryInspector projectId="project-1" />);

    const activeEntry = await screen.findByTestId("memory-active-entry-entry-1");
    expect(within(activeEntry).getByText("Use local approval checkpoints")).toBeInTheDocument();
    expect(within(activeEntry).queryByText("Observation")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /Observations/ }));

    const observation = await screen.findByTestId("memory-observation-observation-1");
    expect(within(observation).getByText("Observation")).toBeInTheDocument();
    expect(within(observation).getByText("Low confidence")).toBeInTheDocument();
    expect(within(observation).getByText("0.35")).toBeInTheDocument();
    expect(within(observation).getByText("Stale file reference")).toBeInTheDocument();
    expect(within(observation).getByText("Quarantined")).toBeInTheDocument();
  });

  it("promotes a pending project candidate", async () => {
    const user = userEvent.setup();
    render(<MemoryInspector projectId="project-1" />);

    const candidate = await screen.findByTestId("memory-candidate-candidate-1");
    await user.click(within(candidate).getByRole("button", { name: "Promote Prefer compact inspector sections" }));

    expect(memoryApi.memoryPromote).toHaveBeenCalledWith("project-1", "candidate-1", "project", false);
  });

  it("edits and promotes a candidate through desktop save and candidate removal", async () => {
    const user = userEvent.setup();
    render(<MemoryInspector projectId="project-1" />);

    const candidate = await screen.findByTestId("memory-candidate-candidate-1");
    expect(within(candidate).getByText("obs-candidate-1")).toBeInTheDocument();

    await user.click(within(candidate).getByRole("button", { name: "Edit and promote Prefer compact inspector sections" }));
    await user.clear(screen.getByLabelText("Title"));
    await user.type(screen.getByLabelText("Title"), "Promoted inspector rule");
    await user.click(screen.getByRole("button", { name: "Promote" }));

    expect(memoryApi.memorySave).toHaveBeenCalledWith("project-1", "project", {
      kind: "preference",
      title: "Promoted inspector rule",
      content: "Use dense sections instead of table-heavy memory UI.",
      files: ["apps/desktop/src/components/Inspector.tsx"],
      concepts: ["desktop", "memory"],
      source_observation_ids: ["obs-candidate-1"],
      pinned: false
    });
    expect(memoryApi.memoryUpdate).toHaveBeenCalledWith("project-1", "candidate-1", "reject", "project");
  });

  it("edits an active entry without dropping concepts or source observations", async () => {
    const user = userEvent.setup();
    render(<MemoryInspector projectId="project-1" />);

    const activeEntry = await screen.findByTestId("memory-active-entry-entry-1");
    await user.click(within(activeEntry).getByRole("button", { name: "Edit Use local approval checkpoints" }));
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(memoryApi.memoryUpdate).toHaveBeenCalledWith(
      "project-1",
      "entry-1",
      "supersede",
      "project",
      "fact",
      "Use local approval checkpoints",
      "Approval rollback is backed by a checkpoint id.",
      [],
      ["desktop", "approval"],
      false,
      ["obs-entry-1"]
    );
  });
});
