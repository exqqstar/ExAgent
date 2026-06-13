import "@testing-library/jest-dom/vitest";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import { exagentClient } from "@/api/exagentClient";
import { Composer } from "@/components/Composer";
import { I18nProvider } from "@/lib/i18n";
import { useWorkbenchStore } from "@/stores/workbenchStore";
import type {
  BackendRuntimeEvent,
  ProviderDescriptor,
  ProviderSettingsResponse,
  ThreadGoal,
  ThreadRecord,
} from "@/types";

const tauriMocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
}));

const dialogMocks = vi.hoisted(() => ({
  open: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: tauriMocks.invoke,
  convertFileSrc: (path: string) => `asset://${path}`,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: tauriMocks.listen,
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: dialogMocks.open,
}));

class ResizeObserverMock {
  observe() {}
  unobserve() {}
  disconnect() {}
}

const deepSeekProvider: ProviderDescriptor = {
  id: "deepseek",
  name: "DeepSeek",
  description: "Use DeepSeek API with an API key",
  recommended: false,
  supported: true,
  auth_mode: "api_key_required",
  protocol: "openai_chat_completions",
  default_base_url: "https://api.deepseek.com",
  default_model: "deepseek-v4-flash",
  supports_model_discovery: true,
  supports_tools: true,
  unsupported_reason: null,
};

function deepSeekProviderSettings(
  overrides: Partial<ProviderSettingsResponse> = {},
): ProviderSettingsResponse {
  return {
    providers: [deepSeekProvider],
    active_provider_id: "deepseek",
    active_credential_id: "key-1",
    credentials: [
      {
        id: "key-1",
        label: "API key 1",
        source: "keychain",
        kind: "api_key",
        status: "active",
        auth_method: null,
        account_label: null,
      },
    ],
    config: {
      provider_id: "deepseek",
      base_url: "https://api.deepseek.com",
      model: "deepseek-v4-flash",
      has_api_key: true,
      credential_source: "keychain",
      auth_required: true,
    },
    connected_provider: {
      id: "deepseek",
      name: "DeepSeek",
      model: "deepseek-v4-flash",
      base_url: "https://api.deepseek.com",
    },
    last_connection: null,
    configured_providers: [
      {
        provider_id: "deepseek",
        base_url: "https://api.deepseek.com",
        model: "deepseek-v4-flash",
        has_api_key: true,
        credential_source: "keychain",
        auth_required: true,
      },
    ],
    model_options: [],
    ...overrides,
  };
}

function oauthProviderSettings(
  providerId: "openai" | "github_copilot",
  name: string,
): ProviderSettingsResponse {
  const provider: ProviderDescriptor = {
    id: providerId,
    name,
    description:
      providerId === "openai"
        ? "Use ChatGPT Pro/Plus or an API key"
        : "Use GitHub Copilot with device OAuth",
    recommended: providerId === "openai",
    supported: true,
    auth_mode: providerId === "openai" ? "api_key_required" : "oauth_required",
    protocol:
      providerId === "openai" ? "openai_chat_completions" : "copilot_oauth",
    default_base_url:
      providerId === "openai"
        ? "https://api.openai.com/v1"
        : "https://api.githubcopilot.com",
    default_model: "gpt-5.5",
    supports_model_discovery: true,
    supports_tools: true,
    unsupported_reason: null,
  };
  return {
    providers: [provider],
    active_provider_id: providerId,
    active_credential_id: providerId === "openai" ? "chatgpt-1" : "copilot-1",
    credentials: [
      {
        id: providerId === "openai" ? "chatgpt-1" : "copilot-1",
        label: providerId === "openai" ? "ChatGPT Pro" : "GitHub Copilot",
        source: "keychain",
        kind: "oauth",
        status: "active",
        auth_method:
          providerId === "openai" ? "chatgpt_oauth" : "github_copilot_oauth",
        account_label: providerId === "openai" ? "user@example.com" : null,
      },
    ],
    config: {
      provider_id: providerId,
      base_url: provider.default_base_url,
      model: provider.default_model,
      has_api_key: false,
      has_credential: true,
      credential_kind: "oauth",
      credential_source: "keychain",
      auth_required: true,
    },
    connected_provider: {
      id: providerId,
      name,
      model: provider.default_model,
      base_url: provider.default_base_url,
    },
    last_connection: null,
    configured_providers: [],
    model_options: [],
  };
}

function threadRecord(overrides: Partial<ThreadRecord> = {}): ThreadRecord {
  return {
    id: "session-record",
    project_id: "project-exagent",
    rollout_path: "/tmp/rollout.jsonl",
    user_title: null,
    fallback_title: "Session record",
    preview: "Session record",
    title_source: "rollout",
    archived_at: null,
    pinned: false,
    status: "idle",
    created_at: 1,
    updated_at: 1,
    last_opened_at: null,
    ...overrides,
  };
}

function threadGoal(overrides: Partial<ThreadGoal> = {}): ThreadGoal {
  return {
    thread_id: "session-desktop",
    goal_id: "goal-desktop",
    objective: "Ship goal mode",
    status: "active",
    token_budget: null,
    tokens_used: 0,
    time_used_seconds: 0,
    continuation_suppressed: false,
    continuation_suppressed_after_turn_id: null,
    created_at_ms: 1,
    updated_at_ms: 1,
    ...overrides,
  };
}

function pendingApproval(overrides: Record<string, unknown> = {}) {
  return {
    thread_id: "thread-alpha",
    approval_id: "approval-alpha",
    kind: "command",
    summary: "Run migration",
    detail: "npm run migrate -- --tenant acme",
    goal_id: "goal-alpha",
    requested_at_ms: 1_718_000_000_000,
    checkpoint_id: "checkpoint-alpha",
    ...overrides,
  };
}

function seedApprovalInbox(approvals: ReturnType<typeof pendingApproval>[]) {
  useWorkbenchStore.setState({
    loading: false,
    activeProjectId: "project-exagent",
    activeSessionId: "session-desktop",
    projects: [
      {
        id: "project-exagent",
        name: "ExAgent",
        path: "/Users/enxiang/dev/ExAgent",
        active: true,
      },
    ],
    sessions: [
      {
        id: "session-desktop",
        projectId: "project-exagent",
        title: "Desktop GUI workbench",
        updatedAt: "local preview",
        status: "awaiting_approval",
      },
    ],
    pendingApprovals: approvals,
    approvalsStatus: "ready",
    approvalsError: null,
  } as any);
}

function createDeferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((innerResolve, innerReject) => {
    resolve = innerResolve;
    reject = innerReject;
  });
  return { promise, resolve, reject };
}

describe("AppShell", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    tauriMocks.invoke.mockReset();
    tauriMocks.listen.mockReset();
    dialogMocks.open.mockReset();
    vi.stubGlobal("ResizeObserver", ResizeObserverMock);
    window.localStorage.clear();
    document.documentElement.removeAttribute("data-theme");
    document.documentElement.lang = "";
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    useWorkbenchStore.setState(useWorkbenchStore.getInitialState(), true);
  });

  it("renders the main desktop workbench regions", async () => {
    render(<App />);

    expect(
      (await screen.findAllByText("Desktop GUI workbench"))[0],
    ).toBeInTheDocument();
    expect(screen.getByText("Project")).toBeInTheDocument();
    const projectButton = screen.getByRole("button", { name: /^ExAgent$/ });
    expect(projectButton).toHaveAttribute("aria-expanded", "true");
    act(() => {
      projectButton.click();
    });
    expect(projectButton).toHaveAttribute("aria-expanded", "false");
    act(() => {
      projectButton.click();
    });
    expect(projectButton).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByLabelText("Inspector")).toBeInTheDocument();
    expect(screen.getByLabelText("Prompt composer")).toBeInTheDocument();
  });

  it("resizes and collapses the desktop project sidebar", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    const sidebar = screen.getByRole("complementary", {
      name: "Projects and sessions",
    });
    const resizeHandle = screen.getByRole("separator", {
      name: "Resize project sidebar",
    });

    expect(sidebar).toHaveStyle({ width: "280px" });

    fireEvent.mouseDown(resizeHandle, { button: 0, clientX: 280 });
    fireEvent.mouseMove(resizeHandle, { clientX: 720 });

    await waitFor(() => {
      expect(sidebar).toHaveStyle({ width: "420px" });
    });

    fireEvent.mouseUp(resizeHandle);
    fireEvent.mouseDown(resizeHandle, { button: 0, clientX: 420 });
    fireEvent.mouseMove(resizeHandle, { clientX: 180 });

    await waitFor(() => {
      expect(
        screen.queryByRole("complementary", { name: "Projects and sessions" }),
      ).not.toBeInTheDocument();
    });

    await userEvent.click(
      screen.getByRole("button", { name: "Show project sidebar" }),
    );

    expect(
      screen.getByRole("complementary", { name: "Projects and sessions" }),
    ).toHaveStyle({ width: "420px" });
  });

  it("renders scaffold transcript labels and runtime notes", async () => {
    render(<App />);

    expect(await screen.findByText("Session restored")).toBeInTheDocument();
    expect(screen.getByText("Changed Files")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Events/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Token Usage/ })).toHaveTextContent("not reported");
    expect(screen.getByText("No token usage reported for this thread.")).toBeInTheDocument();
    expect(screen.getByLabelText("Message ExAgent")).toBeInTheDocument();
  });

  it("renders goal completion reports as transcript cards", async () => {
    const report = {
      goal_id: "goal-desktop",
      objective: "Ship morning report",
      final_status: "complete" as const,
      turns_run: 3,
      tokens_used: 800,
      token_budget: 1000,
      time_used_seconds: 90,
      changed_files: [
        "src/runtime/goal/runtime.rs",
        "apps/desktop/src/components/TranscriptList.tsx",
      ],
      pending_approvals_count: 2,
      summary: "The goal completed after runtime and desktop updates.",
    };
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-desktop",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [
          {
            id: "turn-goal-report",
            status: "completed",
            items: [{ type: "goal_report", event_id: "evt-goal-report", report }],
          },
        ],
      },
    });
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-desktop",
      events: [],
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(null);
    useWorkbenchStore.setState({
      loading: false,
      activeProjectId: "project-exagent",
      activeSessionId: null,
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Users/enxiang/dev/ExAgent",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-desktop",
          projectId: "project-exagent",
          title: "Desktop GUI workbench",
          updatedAt: "local preview",
          status: "idle",
        },
      ],
      transcript: [],
      events: [],
    });

    render(<App />);
    await act(async () => {
      await useWorkbenchStore.getState().openSession("session-desktop");
    });

    expect(await screen.findByRole("article", { name: "Goal report" })).toBeInTheDocument();
    expect(screen.getByText("Goal complete")).toBeInTheDocument();
    expect(screen.getByText("Ship morning report")).toBeInTheDocument();
    expect(screen.getByText("3 turns")).toBeInTheDocument();
    expect(screen.getByText("800 / 1,000 tokens")).toBeInTheDocument();
    expect(screen.getByText("1m 30s")).toBeInTheDocument();
    expect(screen.getByText("src/runtime/goal/runtime.rs")).toBeInTheDocument();
    expect(screen.getByText("apps/desktop/src/components/TranscriptList.tsx")).toBeInTheDocument();
    expect(screen.getByText("2 approvals waiting in Inbox")).toBeInTheDocument();
  });

  it("renders goal reports when empty changed files are omitted", async () => {
    const report = {
      goal_id: "goal-no-files",
      objective: "List available tools",
      final_status: "complete" as const,
      turns_run: 1,
      tokens_used: 0,
      time_used_seconds: 9,
      pending_approvals_count: 0,
      summary: "The goal completed without file changes.",
    };
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-goal-no-files",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [
          {
            id: "turn-goal-no-files",
            status: "completed",
            items: [{ type: "goal_report", event_id: "evt-goal-no-files", report: report as any }],
          },
        ],
      },
    });
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-goal-no-files",
      events: [],
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(null);
    useWorkbenchStore.setState({
      loading: false,
      activeProjectId: "project-exagent",
      activeSessionId: null,
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Users/enxiang/dev/ExAgent",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-goal-no-files",
          projectId: "project-exagent",
          title: "Goal no files",
          updatedAt: "local preview",
          status: "idle",
        },
      ],
      transcript: [],
      events: [],
    });

    render(<App />);
    await act(async () => {
      await useWorkbenchStore.getState().openSession("session-goal-no-files");
    });

    expect(await screen.findByRole("article", { name: "Goal report" })).toBeInTheDocument();
    expect(screen.getByText("List available tools")).toBeInTheDocument();
    expect(screen.getByText("The goal completed without file changes.")).toBeInTheDocument();
  });

  it("lists pending approvals grouped by goal or thread with expandable detail", async () => {
    const user = userEvent.setup();
    seedApprovalInbox([
      pendingApproval({
        thread_id: "thread-goal",
        approval_id: "approval-goal",
        summary: "Run migration",
        detail: "npm run migrate -- --tenant acme",
        goal_id: "goal-alpha",
      }),
      pendingApproval({
        thread_id: "thread-standalone",
        approval_id: "approval-standalone",
        summary: "Patch config",
        detail: "diff --git a/config.ts b/config.ts",
        goal_id: null,
        checkpoint_id: null,
      }),
    ]);

    render(<App />);

    await user.click(
      screen.getByRole("button", {
        name: "Approval inbox, 2 pending approvals",
      }),
    );

    expect(screen.getByText("Goal goal-alpha")).toBeInTheDocument();
    expect(screen.getByText("Thread thread-standalone")).toBeInTheDocument();
    expect(screen.getByText("Run migration")).toBeInTheDocument();
    expect(screen.queryByText("npm run migrate -- --tenant acme")).not.toBeInTheDocument();

    await user.click(
      screen.getByRole("button", {
        name: "Show details for Run migration",
      }),
    );

    expect(screen.getByText("npm run migrate -- --tenant acme")).toBeInTheDocument();
  });

  it("approves one inbox item through the existing approval decision path and refetches it", async () => {
    const user = userEvent.setup();
    seedApprovalInbox([
      pendingApproval({
        thread_id: "thread-approve",
        approval_id: "approval-approve",
        summary: "Install dependency",
        detail: "npm install tiny-package",
      }),
    ]);
    const submitApprovalDecision = vi
      .spyOn(exagentClient, "submitApprovalDecision")
      .mockResolvedValue({} as any);
    vi.spyOn(exagentClient as any, "listApprovals").mockResolvedValue({
      approvals: [],
    });

    render(<App />);
    await user.click(
      screen.getByRole("button", {
        name: "Approval inbox, 1 pending approval",
      }),
    );
    await user.click(screen.getByRole("button", { name: "Approve Install dependency" }));

    expect(submitApprovalDecision).toHaveBeenCalledWith(
      "project-exagent",
      "thread-approve",
      undefined,
      "approval-approve",
      "approved",
      "desktop approved",
    );
    await waitFor(() => {
      expect(screen.queryByText("Install dependency")).not.toBeInTheDocument();
    });
  });

  it("resolves open questions from the approval inbox without approval decision", async () => {
    const user = userEvent.setup();
    seedApprovalInbox([
      pendingApproval({
        thread_id: "thread-question",
        approval_id: "oq_1",
        kind: "open_question",
        summary: "Which cohort ships first?",
        detail: "Blocks: release targeting",
        goal_id: "goal-alpha",
        checkpoint_id: null,
      }),
    ]);
    const resolveOpenQuestion = vi
      .spyOn(exagentClient, "resolveOpenQuestion")
      .mockResolvedValue({} as any);
    const submitApprovalDecision = vi
      .spyOn(exagentClient, "submitApprovalDecision")
      .mockResolvedValue({} as any);
    vi.spyOn(exagentClient as any, "listApprovals").mockResolvedValue({
      approvals: [],
    });

    render(<App />);
    await user.click(
      screen.getByRole("button", {
        name: "Approval inbox, 1 pending approval",
      }),
    );
    await user.type(screen.getByRole("textbox", { name: "Answer Which cohort ships first?" }), "Beta users");
    await user.click(screen.getByRole("button", { name: "Resolve Which cohort ships first?" }));

    expect(resolveOpenQuestion).toHaveBeenCalledWith(
      "project-exagent",
      "thread-question",
      "oq_1",
      "Beta users",
    );
    expect(submitApprovalDecision).not.toHaveBeenCalled();
  });

  it("approves selected inbox items sequentially and reports partial failure", async () => {
    const firstDecision = createDeferred<unknown>();
    const user = userEvent.setup();
    seedApprovalInbox([
      pendingApproval({
        thread_id: "thread-one",
        approval_id: "approval-ok",
        summary: "Run first command",
        detail: "cargo fmt",
      }),
      pendingApproval({
        thread_id: "thread-two",
        approval_id: "approval-fail",
        summary: "Run second command",
        detail: "cargo test",
      }),
    ]);
    const calls: string[] = [];
    vi.spyOn(exagentClient, "submitApprovalDecision").mockImplementation(
      async (_projectId, _threadId, _turnId, approvalId) => {
        calls.push(String(approvalId));
        if (approvalId === "approval-ok") {
          return firstDecision.promise as Promise<any>;
        }
        throw new Error("backend denied approval");
      },
    );
    vi.spyOn(exagentClient as any, "listApprovals").mockResolvedValue({
      approvals: [
        pendingApproval({
          thread_id: "thread-two",
          approval_id: "approval-fail",
          summary: "Run second command",
          detail: "cargo test",
        }),
      ],
    });

    render(<App />);
    await user.click(
      screen.getByRole("button", {
        name: "Approval inbox, 2 pending approvals",
      }),
    );
    await user.click(screen.getByRole("checkbox", { name: "Select Run first command" }));
    await user.click(screen.getByRole("checkbox", { name: "Select Run second command" }));
    fireEvent.click(screen.getByRole("button", { name: "Approve selected approvals" }));

    expect(calls).toEqual(["approval-ok"]);
    firstDecision.resolve({});

    await waitFor(() => {
      expect(calls).toEqual(["approval-ok", "approval-fail"]);
    });
    expect(
      await screen.findByText("Approved 1 of 2 selected approvals. Stopped at approval-fail: backend denied approval"),
    ).toBeInTheDocument();
  });

  it("localizes approval inbox action labels and partial batch status", async () => {
    window.localStorage.setItem("exagent.locale", "zh");
    const firstDecision = createDeferred<unknown>();
    const user = userEvent.setup();
    seedApprovalInbox([
      pendingApproval({
        thread_id: "thread-one",
        approval_id: "approval-ok",
        summary: "Run first command",
        detail: "cargo fmt",
      }),
      pendingApproval({
        thread_id: "thread-two",
        approval_id: "approval-fail",
        summary: "Run second command",
        detail: "cargo test",
      }),
    ]);
    vi.spyOn(exagentClient, "submitApprovalDecision").mockImplementation(
      async (_projectId, _threadId, _turnId, approvalId) => {
        if (approvalId === "approval-ok") {
          return firstDecision.promise as Promise<any>;
        }
        throw new Error("backend denied approval");
      },
    );
    vi.spyOn(exagentClient as any, "listApprovals").mockResolvedValue({
      approvals: [
        pendingApproval({
          thread_id: "thread-two",
          approval_id: "approval-fail",
          summary: "Run second command",
          detail: "cargo test",
        }),
      ],
    });

    render(
      <I18nProvider>
        <App />
      </I18nProvider>,
    );
    await user.click(
      screen.getByRole("button", {
        name: "审批收件箱, 2 待处理 审批",
      }),
    );
    await user.click(screen.getByRole("checkbox", { name: "选择 Run first command" }));
    await user.click(screen.getByRole("checkbox", { name: "选择 Run second command" }));
    await user.click(screen.getByRole("button", { name: "批准所选审批" }));

    firstDecision.resolve({});

    expect(
      await screen.findByText("已批准 1/2 个所选审批。停在 approval-fail：backend denied approval"),
    ).toBeInTheDocument();
    expect(screen.queryByText(/Approved 1 of 2 selected approvals/)).not.toBeInTheDocument();
  });

  it("requires explicit confirmation before rejecting and rolling back to a checkpoint", async () => {
    const user = userEvent.setup();
    seedApprovalInbox([
      pendingApproval({
        thread_id: "thread-rollback",
        approval_id: "approval-rollback",
        summary: "Apply patch",
        detail: "diff --git a/src/main.rs b/src/main.rs",
        checkpoint_id: "checkpoint-rollback",
      }),
    ]);
    const submitApprovalDecision = vi
      .spyOn(exagentClient, "submitApprovalDecision")
      .mockResolvedValue({} as any);
    const restoreCheckpoint = vi
      .spyOn(exagentClient as any, "restoreCheckpoint")
      .mockResolvedValue({ checkpoint_id: "checkpoint-rollback", status: "restored" });
    vi.spyOn(exagentClient as any, "listApprovals").mockResolvedValue({
      approvals: [],
    });

    render(<App />);
    await user.click(
      screen.getByRole("button", {
        name: "Approval inbox, 1 pending approval",
      }),
    );
    await user.click(screen.getByRole("button", { name: "Reject and roll back Apply patch" }));

    const confirm = screen.getByRole("button", { name: "Confirm reject and roll back" });
    expect(confirm).toBeDisabled();
    expect(screen.getByText("checkpoint-rollback")).toBeInTheDocument();
    expect(submitApprovalDecision).not.toHaveBeenCalled();
    expect(restoreCheckpoint).not.toHaveBeenCalled();

    await user.click(screen.getByRole("checkbox", { name: "I understand rollback will restore this checkpoint" }));
    await user.click(confirm);

    await waitFor(() => {
      expect(submitApprovalDecision).toHaveBeenCalledWith(
        "project-exagent",
        "thread-rollback",
        undefined,
        "approval-rollback",
        "denied",
        "desktop denied",
      );
    });
    expect(restoreCheckpoint).toHaveBeenCalledWith("project-exagent", "checkpoint-rollback");
    expect(submitApprovalDecision.mock.invocationCallOrder[0]).toBeLessThan(
      restoreCheckpoint.mock.invocationCallOrder[0],
    );
  });

  it("removes a denied rollback item and reports when checkpoint restore fails", async () => {
    const user = userEvent.setup();
    seedApprovalInbox([
      pendingApproval({
        thread_id: "thread-rollback",
        approval_id: "approval-rollback-fails",
        summary: "Apply risky patch",
        detail: "diff --git a/src/main.rs b/src/main.rs",
        checkpoint_id: "checkpoint-rollback-fails",
      }),
    ]);
    vi.spyOn(exagentClient, "submitApprovalDecision").mockResolvedValue({} as any);
    vi.spyOn(exagentClient as any, "restoreCheckpoint").mockRejectedValue(new Error("restore exploded"));
    vi.spyOn(exagentClient as any, "listApprovals").mockResolvedValue({
      approvals: [],
    });

    render(<App />);
    await user.click(
      screen.getByRole("button", {
        name: "Approval inbox, 1 pending approval",
      }),
    );
    await user.click(screen.getByRole("button", { name: "Reject and roll back Apply risky patch" }));
    await user.click(screen.getByRole("checkbox", { name: "I understand rollback will restore this checkpoint" }));
    await user.click(screen.getByRole("button", { name: "Confirm reject and roll back" }));

    expect(
      await screen.findByText("Rejected approval-rollback-fails, but rollback failed: restore exploded"),
    ).toBeInTheDocument();
    expect(screen.queryByText("Apply risky patch")).not.toBeInTheDocument();
  });

  it("shows an inbox badge when pending approvals are refreshed from runtime events", async () => {
    vi.useFakeTimers();
    try {
      seedApprovalInbox([]);
      vi.spyOn(exagentClient as any, "listApprovals").mockResolvedValue({
        approvals: [
          pendingApproval({
            thread_id: "thread-event",
            approval_id: "approval-event",
            summary: "Run event command",
            detail: "npm test",
          }),
        ],
      });

      render(<App />);

      await act(async () => {
        useWorkbenchStore.getState().applyRuntimeEvent({
          event_id: "evt-approval-requested",
          thread_id: "session-desktop",
          turn_id: "turn-event",
          kind: {
            type: "approval_requested",
            approval_id: "approval-event",
            tool_name: "run_command",
            reason: "Needs permission",
            checkpoint_id: "checkpoint-event",
          },
        });
        vi.advanceTimersByTime(300);
        await Promise.resolve();
      });

      expect(
        screen.getByRole("button", {
          name: "Approval inbox, 1 pending approval",
        }),
      ).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows an inbox badge when open questions are refreshed from runtime events", async () => {
    vi.useFakeTimers();
    try {
      seedApprovalInbox([]);
      vi.spyOn(exagentClient as any, "listApprovals").mockResolvedValue({
        approvals: [
          pendingApproval({
            thread_id: "thread-event",
            approval_id: "oq-event",
            kind: "open_question",
            summary: "Which cohort ships first?",
            detail: "Blocks: Release targeting",
          }),
        ],
      });

      render(<App />);

      await act(async () => {
        useWorkbenchStore.getState().applyRuntimeEvent({
          event_id: "evt-open-question-recorded",
          thread_id: "session-desktop",
          turn_id: "turn-event",
          kind: {
            type: "open_question_recorded",
            question_id: "oq-event",
            goal_id: "goal-event",
            question: "Which cohort ships first?",
            blocks_what: "Release targeting",
          },
        });
        vi.advanceTimersByTime(300);
        await Promise.resolve();
      });

      expect(
        screen.getByRole("button", {
          name: "Approval inbox, 1 pending approval",
        }),
      ).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("expands an inactive project without switching the active project", async () => {
    const user = userEvent.setup();
    vi.spyOn(exagentClient, "reindexProject").mockResolvedValue([
      threadRecord({
        id: "session-beta",
        project_id: "project-beta",
        fallback_title: "Beta session",
      }),
    ]);
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-alpha",
          name: "Alpha",
          path: "/tmp/alpha",
          active: true,
        },
        { id: "project-beta", name: "Beta", path: "/tmp/beta", active: false },
      ],
      sessions: [
        {
          id: "session-alpha",
          projectId: "project-alpha",
          title: "Alpha session",
          updatedAt: "now",
          status: "idle",
        },
      ],
      activeProjectId: "project-alpha",
      activeSessionId: "session-alpha",
    });

    render(<App />);
    await user.click(screen.getByRole("button", { name: /^Beta$/ }));

    expect(await screen.findByText("Beta session")).toBeInTheDocument();
    expect(useWorkbenchStore.getState().activeProjectId).toBe("project-alpha");
    expect(useWorkbenchStore.getState().activeSessionId).toBe("session-alpha");
  });

  it("opens the clicked session when choosing from an inactive expanded project", async () => {
    const user = userEvent.setup();
    vi.spyOn(exagentClient, "reindexProject").mockResolvedValue([
      threadRecord({
        id: "session-beta",
        project_id: "project-beta",
        fallback_title: "Beta session",
      }),
    ]);
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-beta",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [
          {
            id: "turn-beta",
            status: "completed",
            items: [{ type: "assistant_message", text: "Beta transcript" }],
          },
        ],
      },
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(null);
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-beta",
      events: [],
    });
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-alpha",
          name: "Alpha",
          path: "/tmp/alpha",
          active: true,
        },
        { id: "project-beta", name: "Beta", path: "/tmp/beta", active: false },
      ],
      sessions: [
        {
          id: "session-alpha",
          projectId: "project-alpha",
          title: "Alpha session",
          updatedAt: "now",
          status: "idle",
        },
      ],
      activeProjectId: "project-alpha",
      activeSessionId: "session-alpha",
    });

    render(<App />);
    await user.click(screen.getByRole("button", { name: /^Beta$/ }));
    await user.click(await screen.findByText("Beta session"));

    await waitFor(() => {
      expect(useWorkbenchStore.getState().activeProjectId).toBe("project-beta");
      expect(useWorkbenchStore.getState().activeSessionId).toBe("session-beta");
    });
    expect(useWorkbenchStore.getState().transcript).toEqual([
      expect.objectContaining({
        body: "Beta transcript",
        threadId: "session-beta",
      }),
    ]);
  });

  it("shows token usage from replayed root token count events", async () => {
    vi.spyOn(exagentClient, "getWorkbenchSnapshot").mockResolvedValue({
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Volumes/EXEXEX/ExAgent",
          active: true
        }
      ],
      sessions: [
        {
          id: "session-token",
          projectId: "project-exagent",
          title: "Token session",
          updatedAt: "now",
          status: "idle"
        }
      ],
      activeProjectId: "project-exagent",
      activeSessionId: "session-token",
      transcript: [],
      events: [],
      changedFiles: [],
      cwd: "/Volumes/EXEXEX/ExAgent",
      policy: "local",
      tokenUsage: {
        input: 0,
        output: 0,
        limit: 1
      },
      tokenUsageByThreadId: {},
      runtimeSettings: null,
      selectedModel: null,
      selectedThinkingMode: null
    });
    vi.spyOn(exagentClient, "getRuntimeSettings").mockResolvedValue({
      default_model: "gpt-5.5",
      default_thinking_mode: "medium",
      presets: [],
      mcp_servers: [],
      skill_roots: []
    });
    vi.spyOn(exagentClient, "getProviderSettings").mockResolvedValue(deepSeekProviderSettings());
    vi.spyOn(exagentClient, "reindexProject").mockResolvedValue([
      threadRecord({
        id: "session-token",
        fallback_title: "Token session"
      })
    ]);
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-token",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: []
      }
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(null);
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-token",
      events: [
        {
          event_id: "evt-token-session",
          thread_id: "session-token",
          turn_id: "turn-token",
          kind: {
            type: "token_count",
            info: {
              total_token_usage: {
                input_tokens: 142000,
                cached_input_tokens: 28000,
                output_tokens: 31200,
                reasoning_output_tokens: 13200,
                total_tokens: 186400
              },
              last_token_usage: {
                input_tokens: 52000,
                cached_input_tokens: 8000,
                output_tokens: 6200,
                reasoning_output_tokens: 1200,
                total_tokens: 59400
              },
              model_context_window: 400000
            }
          }
        }
      ]
    });
    Reflect.set(window, "__TAURI_INTERNALS__", {});

    render(<App />);

    expect(await screen.findAllByText("Token session")).not.toHaveLength(0);
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /Token Usage/ })).toHaveTextContent("186.4k tokens");
    });
    expect(screen.getByText("186,400")).toBeInTheDocument();
    expect(screen.queryByText(/0% context/i)).not.toBeInTheDocument();
  });

  it("starts a draft session from an inactive project action without opening old sessions", async () => {
    const user = userEvent.setup();
    const openSession = vi.spyOn(useWorkbenchStore.getState(), "openSession");
    vi.spyOn(exagentClient, "reindexProject").mockResolvedValue([
      threadRecord({
        id: "session-beta",
        project_id: "project-beta",
        fallback_title: "Beta session",
      }),
    ]);
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-alpha",
          name: "Alpha",
          path: "/tmp/alpha",
          active: true,
        },
        { id: "project-beta", name: "Beta", path: "/tmp/beta", active: false },
      ],
      sessions: [
        {
          id: "session-alpha",
          projectId: "project-alpha",
          title: "Alpha session",
          updatedAt: "now",
          status: "idle",
        },
      ],
      activeProjectId: "project-alpha",
      activeSessionId: "session-alpha",
      transcript: [
        {
          id: "message-alpha",
          role: "assistant",
          body: "Alpha transcript",
          timestamp: "now",
        },
      ],
    });

    render(<App />);
    await user.click(
      screen.getByRole("button", { name: "New session for Beta" }),
    );

    await waitFor(() => {
      expect(useWorkbenchStore.getState()).toMatchObject({
        activeProjectId: "project-beta",
        activeSessionId: null,
        cwd: "/tmp/beta",
        transcript: [],
      });
    });
    expect(openSession).not.toHaveBeenCalled();
    expect(
      screen.getByText("What should we build in Beta?"),
    ).toBeInTheDocument();
  });

  it("opens project action menu without switching the active project", async () => {
    const user = userEvent.setup();
    const revealProjectInFileManager = vi
      .spyOn(exagentClient, "revealProjectInFileManager")
      .mockResolvedValue(undefined);
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-alpha",
          name: "Alpha",
          path: "/tmp/alpha",
          active: true,
        },
        { id: "project-beta", name: "Beta", path: "/tmp/beta", active: false },
      ],
      sessions: [
        {
          id: "session-alpha",
          projectId: "project-alpha",
          title: "Alpha session",
          updatedAt: "now",
          status: "idle",
        },
      ],
      activeProjectId: "project-alpha",
      activeSessionId: "session-alpha",
    });

    render(<App />);
    await user.click(
      screen.getByRole("button", { name: "Project actions for Beta" }),
    );

    expect(await screen.findByText("Show in Finder")).toBeInTheDocument();
    expect(screen.getByText("Create permanent worktree")).toBeInTheDocument();
    expect(screen.getByText("Rename project")).toBeInTheDocument();
    expect(useWorkbenchStore.getState().activeProjectId).toBe("project-alpha");

    await user.click(screen.getByRole("menuitem", { name: /Show in Finder/ }));

    expect(revealProjectInFileManager).toHaveBeenCalledWith("/tmp/beta");
    expect(useWorkbenchStore.getState().activeProjectId).toBe("project-alpha");
  });

  it("pins a project from the action menu without switching projects", async () => {
    const user = userEvent.setup();
    const pinProject = vi.spyOn(exagentClient, "pinProject").mockResolvedValue({
      id: "project-beta",
      name: "Beta",
      path: "/tmp/beta",
      archived_at: null,
      pinned: true,
    });
    vi.spyOn(exagentClient, "listProjects").mockResolvedValue([
      {
        id: "project-beta",
        name: "Beta",
        path: "/tmp/beta",
        archived_at: null,
        pinned: true,
      },
      {
        id: "project-alpha",
        name: "Alpha",
        path: "/tmp/alpha",
        archived_at: null,
        pinned: false,
      },
    ]);
    vi.spyOn(exagentClient, "reindexProject").mockResolvedValue([]);
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-alpha",
          name: "Alpha",
          path: "/tmp/alpha",
          active: true,
        },
        { id: "project-beta", name: "Beta", path: "/tmp/beta", active: false },
      ],
      sessions: [],
      activeProjectId: "project-alpha",
      activeSessionId: null,
    });

    render(<App />);
    await user.click(
      screen.getByRole("button", { name: "Project actions for Beta" }),
    );
    await user.click(
      await screen.findByRole("menuitem", { name: /Pin project/ }),
    );

    expect(pinProject).toHaveBeenCalledWith("project-beta", true);
    await waitFor(() => {
      expect(useWorkbenchStore.getState().activeProjectId).toBe(
        "project-alpha",
      );
    });
  });

  it("renames a project from the action menu", async () => {
    const user = userEvent.setup();
    const renameProject = vi
      .spyOn(exagentClient, "renameProject")
      .mockResolvedValue({
        id: "project-beta",
        name: "Beta Renamed",
        path: "/tmp/beta",
        archived_at: null,
        pinned: false,
      });
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-alpha",
          name: "Alpha",
          path: "/tmp/alpha",
          active: true,
        },
        { id: "project-beta", name: "Beta", path: "/tmp/beta", active: false },
      ],
      sessions: [],
      activeProjectId: "project-alpha",
      activeSessionId: null,
    });

    render(<App />);
    await user.click(
      screen.getByRole("button", { name: "Project actions for Beta" }),
    );
    await user.click(
      await screen.findByRole("menuitem", { name: /Rename project/ }),
    );
    await user.clear(screen.getByLabelText("Project name"));
    await user.type(screen.getByLabelText("Project name"), "Beta Renamed");
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(renameProject).toHaveBeenCalledWith("project-beta", "Beta Renamed");
    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: /^Beta Renamed$/ }),
      ).toBeInTheDocument();
    });
  });

  it("archives all conversations in the active project", async () => {
    const user = userEvent.setup();
    const archiveProjectConversations = vi
      .spyOn(exagentClient, "archiveProjectConversations")
      .mockResolvedValue(undefined);
    vi.spyOn(exagentClient, "listThreads").mockResolvedValue([]);
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-alpha",
          name: "Alpha",
          path: "/tmp/alpha",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-alpha",
          projectId: "project-alpha",
          title: "Alpha session",
          updatedAt: "now",
          status: "idle",
        },
      ],
      activeProjectId: "project-alpha",
      activeSessionId: "session-alpha",
      transcript: [
        {
          id: "message-alpha",
          role: "assistant",
          body: "Alpha transcript",
          timestamp: "now",
        },
      ],
    });

    render(<App />);
    await user.click(
      screen.getByRole("button", { name: "Project actions for Alpha" }),
    );
    await user.click(
      await screen.findByRole("menuitem", { name: /Archive conversations/ }),
    );
    await user.click(
      screen.getByRole("button", { name: "Archive conversations" }),
    );

    expect(archiveProjectConversations).toHaveBeenCalledWith("project-alpha");
    await waitFor(() => {
      expect(useWorkbenchStore.getState().sessions).toEqual([]);
      expect(useWorkbenchStore.getState().activeSessionId).toBeNull();
      expect(useWorkbenchStore.getState().transcript).toEqual([]);
    });
  });

  it("selects the next project after archiving the active project", async () => {
    const user = userEvent.setup();
    vi.spyOn(exagentClient, "archiveProject").mockResolvedValue(undefined);
    vi.spyOn(exagentClient, "listProjects").mockResolvedValue([
      {
        id: "project-beta",
        name: "Beta",
        path: "/tmp/beta",
        archived_at: null,
        pinned: false,
      },
    ]);
    vi.spyOn(exagentClient, "reindexProject").mockResolvedValue([]);
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-alpha",
          name: "Alpha",
          path: "/tmp/alpha",
          active: true,
        },
        { id: "project-beta", name: "Beta", path: "/tmp/beta", active: false },
      ],
      sessions: [],
      activeProjectId: "project-alpha",
      activeSessionId: null,
    });

    render(<App />);
    await user.click(
      screen.getByRole("button", { name: "Project actions for Alpha" }),
    );
    await user.click(
      await screen.findByRole("menuitem", { name: /Archive project/ }),
    );
    await user.click(screen.getByRole("button", { name: "Archive project" }));

    await waitFor(() => {
      expect(useWorkbenchStore.getState().activeProjectId).toBe("project-beta");
      expect(useWorkbenchStore.getState().cwd).toBe("/tmp/beta");
    });
  });

  it("enters no-project state after removing the only project", async () => {
    const user = userEvent.setup();
    vi.spyOn(exagentClient, "removeProject").mockResolvedValue(undefined);
    vi.spyOn(exagentClient, "listProjects").mockResolvedValue([]);
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-alpha",
          name: "Alpha",
          path: "/tmp/alpha",
          active: true,
        },
      ],
      sessions: [],
      activeProjectId: "project-alpha",
      activeSessionId: null,
      cwd: "/tmp/alpha",
    });

    render(<App />);
    await user.click(
      screen.getByRole("button", { name: "Project actions for Alpha" }),
    );
    await user.click(
      await screen.findByRole("menuitem", { name: /Remove from sidebar/ }),
    );
    await user.click(
      screen.getByRole("button", { name: "Remove from sidebar" }),
    );

    await waitFor(() => {
      expect(useWorkbenchStore.getState().activeProjectId).toBeNull();
      expect(useWorkbenchStore.getState().cwd).toBe("No project selected");
    });
  });

  it("shows an add-project state instead of a draft composer without an active project", async () => {
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [],
      sessions: [],
      activeProjectId: null,
      activeSessionId: null,
      transcript: [],
      cwd: "No project selected",
    });

    render(<App />);

    const main = within(screen.getByRole("main"));
    expect(main.getByRole("heading", { name: "Add a project" })).toBeInTheDocument();
    expect(main.getByRole("button", { name: "Add project" })).toBeInTheDocument();
    expect(screen.queryByText("What should we build in ExAgent?")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Prompt composer")).not.toBeInTheDocument();
  });

  it("archives a session from the quick hover action without opening it", async () => {
    const user = userEvent.setup();
    const archiveThread = vi
      .spyOn(exagentClient, "archiveThread")
      .mockResolvedValue(undefined);
    const openSession = vi.spyOn(useWorkbenchStore.getState(), "openSession");
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-alpha",
          name: "Alpha",
          path: "/tmp/alpha",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-alpha",
          projectId: "project-alpha",
          title: "Alpha session",
          updatedAt: "now",
          status: "idle",
        },
      ],
      activeProjectId: "project-alpha",
      activeSessionId: "session-alpha",
      transcript: [
        {
          id: "message-alpha",
          role: "assistant",
          body: "Alpha transcript",
          timestamp: "now",
        },
      ],
    });

    render(<App />);
    await user.click(
      screen.getByRole("button", { name: "Archive Alpha session" }),
    );

    expect(archiveThread).toHaveBeenCalledWith("session-alpha");
    expect(openSession).not.toHaveBeenCalled();
    await waitFor(() => {
      expect(useWorkbenchStore.getState().activeSessionId).toBeNull();
      expect(useWorkbenchStore.getState().transcript).toEqual([]);
    });
  });

  it("forks a historical assistant reply and opens the new thread", async () => {
    const user = userEvent.setup();
    const forkThread = vi.spyOn(exagentClient, "forkThread").mockResolvedValue({
      new_thread_id: "session-fork",
      parent_thread_id: "session-parent",
      fork_point_turn_id: "turn-1",
    });
    vi.spyOn(exagentClient, "listThreads").mockResolvedValue([
      threadRecord({
        id: "session-parent",
        fallback_title: "Parent session",
        created_at: 1,
        updated_at: 1,
      }),
      threadRecord({
        id: "session-fork",
        fallback_title: "Forked session",
        created_at: 2,
        updated_at: 2,
        fork_parent_thread_id: "session-parent",
        fork_point_turn_id: "turn-1",
      }),
    ]);
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-fork",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [
          {
            id: "turn-1",
            status: "completed",
            items: [{ type: "assistant_message", text: "Fork transcript" }],
          },
        ],
      },
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(null);
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-fork",
      events: [],
    });
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Users/enxiang/dev/ExAgent",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-parent",
          projectId: "project-exagent",
          title: "Parent session",
          updatedAt: "now",
          status: "idle",
        },
      ],
      activeProjectId: "project-exagent",
      activeSessionId: "session-parent",
      transcript: [
        {
          id: "message-turn-1",
          role: "user",
          body: "Fork this point",
          timestamp: "history",
          threadId: "session-parent",
          turnId: "turn-1",
          turnStatus: "completed",
        },
        {
          id: "message-assistant-1",
          role: "assistant",
          body: "Parent answer",
          timestamp: "history",
          threadId: "session-parent",
          turnId: "turn-1",
          turnStatus: "completed",
        },
      ],
    });

    render(<App />);

    const userMessage = screen.getByRole("article", { name: "User message" });
    await user.hover(userMessage);
    expect(within(userMessage).queryByRole("button", { name: "Fork from this reply" })).not.toBeInTheDocument();

    await user.hover(screen.getByRole("article", { name: "Assistant message" }));
    await user.click(screen.getByRole("button", { name: "Fork from this reply" }));

    expect(forkThread).toHaveBeenCalledWith("project-exagent", {
      threadId: "session-parent",
      atTurnId: "turn-1",
    });
    await waitFor(() => {
      expect(useWorkbenchStore.getState().activeSessionId).toBe("session-fork");
    });
    expect(screen.getByText("Fork transcript")).toBeInTheDocument();
  });

  it("makes a live completed assistant reply forkable without reopening", async () => {
    const user = userEvent.setup();
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "session-live",
      turn: {
        id: "turn-live",
        status: "in_progress",
        items: [],
      },
    });
    const forkThread = vi.spyOn(exagentClient, "forkThread").mockResolvedValue({
      new_thread_id: "session-live-fork",
      parent_thread_id: "session-live",
      fork_point_turn_id: "turn-live",
    });
    vi.spyOn(exagentClient, "listThreads").mockResolvedValue([
      threadRecord({
        id: "session-live",
        fallback_title: "Live session",
        created_at: 1,
        updated_at: 1,
      }),
      threadRecord({
        id: "session-live-fork",
        fallback_title: "Live fork",
        created_at: 2,
        updated_at: 2,
        fork_parent_thread_id: "session-live",
        fork_point_turn_id: "turn-live",
      }),
    ]);
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-live-fork",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [],
      },
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(null);
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-live-fork",
      events: [],
    });
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Users/enxiang/dev/ExAgent",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-live",
          projectId: "project-exagent",
          title: "Live session",
          updatedAt: "now",
          status: "idle",
        },
      ],
      activeProjectId: "project-exagent",
      activeSessionId: "session-live",
      composerValue: "Live fork prompt",
      transcript: [],
    });

    render(<App />);
    await act(async () => {
      await useWorkbenchStore.getState().sendPrompt();
    });
    expect(startTurn).toHaveBeenCalled();

    expect(screen.queryByRole("button", { name: "Fork from this reply" })).not.toBeInTheDocument();

    act(() => {
      useWorkbenchStore.getState().applyRuntimeEvent({
        event_id: "evt-live-assistant",
        thread_id: "session-live",
        turn_id: "turn-live",
        kind: {
          type: "assistant_turn",
          turn: {
            text: "Live answer",
            tool_calls: [],
          },
        },
      });
    });

    act(() => {
      useWorkbenchStore.getState().applyRuntimeEvent({
        event_id: "evt-live-completed",
        thread_id: "session-live",
        turn_id: "turn-live",
        kind: { type: "turn_completed" },
      });
    });

    await user.hover(screen.getByRole("article", { name: "Assistant message" }));
    const forkButton = screen.getByRole("button", { name: "Fork from this reply" });
    expect(forkButton).toBeEnabled();

    await user.click(forkButton);

    expect(forkThread).toHaveBeenCalledWith("project-exagent", {
      threadId: "session-live",
      atTurnId: "turn-live",
    });
  });

  it("clears active search so a new fork stays visible when opened", async () => {
    const forkThread = vi.spyOn(exagentClient, "forkThread").mockResolvedValue({
      new_thread_id: "session-search-fork",
      parent_thread_id: "session-search-parent",
      fork_point_turn_id: "turn-1",
    });
    const listThreads = vi.spyOn(exagentClient, "listThreads").mockImplementation(async (_projectId, _archived, search) => {
      if (search) {
        return [
          threadRecord({
            id: "session-search-parent",
            fallback_title: "Parent matches search",
            created_at: 1,
            updated_at: 1,
          }),
        ];
      }
      return [
        threadRecord({
          id: "session-search-parent",
          fallback_title: "Parent matches search",
          created_at: 1,
          updated_at: 1,
        }),
        threadRecord({
          id: "session-search-fork",
          fallback_title: "New fork title",
          created_at: 2,
          updated_at: 2,
          fork_parent_thread_id: "session-search-parent",
          fork_point_turn_id: "turn-1",
        }),
      ];
    });
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-search-fork",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [],
      },
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(null);
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-search-fork",
      events: [],
    });
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      search: "parent-only",
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Users/enxiang/dev/ExAgent",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-search-parent",
          projectId: "project-exagent",
          title: "Parent matches search",
          updatedAt: "now",
          status: "idle",
        },
      ],
      activeProjectId: "project-exagent",
      activeSessionId: "session-search-parent",
    });

    await useWorkbenchStore.getState().forkThreadFromTurn("session-search-parent", "turn-1");

    expect(forkThread).toHaveBeenCalledWith("project-exagent", {
      threadId: "session-search-parent",
      atTurnId: "turn-1",
    });
    expect(listThreads).toHaveBeenCalledWith("project-exagent", false, null);
    expect(useWorkbenchStore.getState().search).toBe("");
    expect(useWorkbenchStore.getState().activeSessionId).toBe("session-search-fork");
    expect(useWorkbenchStore.getState().sessions).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          id: "session-search-fork",
          title: "New fork title",
        }),
      ]),
    );
  });

  it("does not offer fork for an in-progress active turn", async () => {
    const user = userEvent.setup();
    const forkThread = vi.spyOn(exagentClient, "forkThread").mockResolvedValue({
      new_thread_id: "session-fork",
      parent_thread_id: "session-active",
      fork_point_turn_id: "turn-active",
    });
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-active",
        status: "running",
        goal_mode: "standard",
        active_turn: {
          id: "turn-active",
          status: "running",
          items: [{ type: "user_message", text: "Still running" }],
        },
        turns: [
          {
            id: "turn-active",
            status: "running",
            items: [{ type: "user_message", text: "Still running" }],
          },
        ],
      },
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(null);
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-active",
      events: [],
    });
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Users/enxiang/dev/ExAgent",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-active",
          projectId: "project-exagent",
          title: "Active session",
          updatedAt: "now",
          status: "running",
        },
      ],
      activeProjectId: "project-exagent",
      activeSessionId: null,
      transcript: [],
    });

    render(<App />);
    await act(async () => {
      await useWorkbenchStore.getState().openSession("session-active");
    });

    await user.hover(screen.getByRole("article", { name: "User message" }));

    expect(screen.queryByRole("button", { name: "Fork from this reply" })).not.toBeInTheDocument();
    expect(forkThread).not.toHaveBeenCalled();
  });

  it("keeps fork disabled while an awaiting-approval turn is active", async () => {
    const user = userEvent.setup();
    const forkThread = vi.spyOn(exagentClient, "forkThread").mockResolvedValue({
      new_thread_id: "session-fork",
      parent_thread_id: "session-awaiting",
      fork_point_turn_id: "turn-completed",
    });
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-awaiting",
        status: "waiting_approval",
        goal_mode: "standard",
        active_turn: {
          id: "turn-awaiting",
          status: "waiting_approval",
          items: [],
        },
        turns: [
          {
            id: "turn-completed",
            status: "completed",
            items: [
              { type: "user_message", text: "Completed prompt" },
              { type: "assistant_message", text: "Completed answer" },
            ],
          },
        ],
      },
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(null);
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-awaiting",
      events: [],
    });
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Users/enxiang/dev/ExAgent",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-awaiting",
          projectId: "project-exagent",
          title: "Awaiting session",
          updatedAt: "now",
          status: "awaiting_approval",
        },
      ],
      activeProjectId: "project-exagent",
      activeSessionId: null,
      transcript: [],
    });

    render(<App />);
    await act(async () => {
      await useWorkbenchStore.getState().openSession("session-awaiting");
    });

    await user.hover(screen.getByRole("article", { name: "Assistant message" }));
    const forkButton = screen.getByRole("button", { name: "Fork from this reply" });

    expect(forkButton).toBeDisabled();
    await user.click(forkButton);
    expect(forkThread).not.toHaveBeenCalled();
  });

  it("renders forked sessions under their parent with a branch label", () => {
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Users/enxiang/dev/ExAgent",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-parent",
          projectId: "project-exagent",
          title: "Parent session",
          updatedAt: "now",
          status: "idle",
          createdAt: 1,
        },
        {
          id: "session-child",
          projectId: "project-exagent",
          title: "Child fork",
          updatedAt: "now",
          status: "idle",
          createdAt: 2,
          forkParentThreadId: "session-parent",
          forkPointTurnId: "turn-1",
        },
        {
          id: "session-flat",
          projectId: "project-exagent",
          title: "Standalone session",
          updatedAt: "now",
          status: "idle",
          createdAt: 3,
        },
      ],
      activeProjectId: "project-exagent",
      activeSessionId: "session-parent",
    });

    render(<App />);

    const sidebar = screen.getByRole("complementary", {
      name: "Projects and sessions",
    });
    const parent = within(sidebar).getByText("Parent session");
    const child = within(sidebar).getByText("Child fork");
    expect(parent.compareDocumentPosition(child) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByText("forked from turn 1")).toBeInTheDocument();
    expect(screen.getByLabelText("Forked session Child fork, forked from turn 1")).toBeInTheDocument();
    expect(screen.getByText("forked from turn 1").closest("[data-session-branch-group]")).toHaveClass(
      "border-l",
      "pl-2.5",
    );
  });

  it("compares a forked session with its parent in read-only panes", async () => {
    const user = userEvent.setup();
    vi.spyOn(exagentClient, "readThread").mockImplementation(async (_projectId, threadId) => {
      if (threadId === "session-parent") {
        return {
          thread: {
            id: "session-parent",
            status: "idle",
            goal_mode: "standard",
            active_turn: null,
            turns: [
              {
                id: "turn-1",
                status: "completed",
                items: [
                  { type: "user_message", text: "Shared prompt" },
                  { type: "assistant_message", text: "Shared answer" },
                ],
              },
              {
                id: "turn-2-parent",
                status: "completed",
                items: [{ type: "assistant_message", text: "Parent-only follow-up" }],
              },
            ],
          },
        };
      }
      return {
        thread: {
          id: "session-child",
          status: "idle",
          goal_mode: "standard",
          active_turn: null,
          turns: [
            {
              id: "turn-1",
              status: "completed",
              items: [
                { type: "user_message", text: "Shared prompt" },
                { type: "assistant_message", text: "Shared answer" },
              ],
            },
            {
              id: "turn-2-child",
              status: "completed",
              items: [{ type: "assistant_message", text: "Fork-only follow-up" }],
            },
          ],
        },
      };
    });
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Users/enxiang/dev/ExAgent",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-parent",
          projectId: "project-exagent",
          title: "Parent session",
          updatedAt: "now",
          status: "idle",
          createdAt: 1,
        },
        {
          id: "session-child",
          projectId: "project-exagent",
          title: "Child fork",
          updatedAt: "now",
          status: "idle",
          createdAt: 2,
          forkParentThreadId: "session-parent",
          forkPointTurnId: "turn-1",
        },
      ],
      activeProjectId: "project-exagent",
      activeSessionId: "session-child",
      transcript: [
        {
          id: "child-current",
          role: "assistant",
          body: "Fork-only follow-up",
          timestamp: "history",
          threadId: "session-child",
          turnId: "turn-2-child",
          turnStatus: "completed",
        },
      ],
    });

    render(<App />);
    await user.click(screen.getByRole("button", { name: "Session actions for Child fork" }));
    await user.click(screen.getByRole("menuitem", { name: "Compare with parent" }));

    const parentPane = await screen.findByLabelText("Parent branch transcript");
    const forkPane = screen.getByLabelText("Fork branch transcript");
    expect(parentPane.compareDocumentPosition(forkPane) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByText("1 shared turn")).toBeInTheDocument();
    expect(within(parentPane).getByText("Parent-only follow-up")).toBeInTheDocument();
    expect(within(forkPane).getByText("Fork-only follow-up")).toBeInTheDocument();
    expect(screen.queryByLabelText("Message ExAgent")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Fork from this reply" })).not.toBeInTheDocument();

    await user.keyboard("{Escape}");

    expect(screen.getByLabelText("Message ExAgent")).toBeInTheDocument();
    expect(screen.queryByLabelText("Parent branch transcript")).not.toBeInTheDocument();
  });

  it("renders approval messages in branch compare without approval controls", async () => {
    const user = userEvent.setup();
    vi.spyOn(exagentClient, "readThread").mockImplementation(async (_projectId, threadId) => {
      if (threadId === "session-parent") {
        return {
          thread: {
            id: "session-parent",
            status: "waiting_approval",
            goal_mode: "standard",
            active_turn: null,
            turns: [
              {
                id: "turn-1",
                status: "completed",
                items: [{ type: "user_message", text: "Shared prompt" }],
              },
              {
                id: "turn-approval",
                status: "waiting_approval",
                items: [
                  {
                    type: "approval_requested",
                    event_id: "evt-parent-approval",
                    approval_id: "approval-parent",
                    tool_name: "run_deploy",
                    reason: "Run parent deployment",
                  },
                ],
              },
            ],
          },
        };
      }
      return {
        thread: {
          id: "session-child",
          status: "idle",
          goal_mode: "standard",
          active_turn: null,
          turns: [
            {
              id: "turn-1",
              status: "completed",
              items: [{ type: "user_message", text: "Shared prompt" }],
            },
            {
              id: "turn-child",
              status: "completed",
              items: [{ type: "assistant_message", text: "Fork-only follow-up" }],
            },
          ],
        },
      };
    });
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Users/enxiang/dev/ExAgent",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-parent",
          projectId: "project-exagent",
          title: "Parent session",
          updatedAt: "now",
          status: "awaiting_approval",
          createdAt: 1,
        },
        {
          id: "session-child",
          projectId: "project-exagent",
          title: "Child fork",
          updatedAt: "now",
          status: "idle",
          createdAt: 2,
          forkParentThreadId: "session-parent",
          forkPointTurnId: "turn-1",
        },
      ],
      activeProjectId: "project-exagent",
      activeSessionId: "session-child",
      transcript: [
        {
          id: "child-current",
          role: "assistant",
          body: "Fork-only follow-up",
          timestamp: "history",
          threadId: "session-child",
          turnId: "turn-child",
          turnStatus: "completed",
        },
      ],
    });

    render(<App />);
    await user.click(screen.getByRole("button", { name: "Session actions for Child fork" }));
    await user.click(screen.getByRole("menuitem", { name: "Compare with parent" }));

    const parentPane = await screen.findByLabelText("Parent branch transcript");
    expect(within(parentPane).getByText("Run parent deployment")).toBeInTheDocument();
    expect(within(parentPane).queryByRole("button", { name: "Approve" })).not.toBeInTheDocument();
    expect(within(parentPane).queryByRole("button", { name: "Deny" })).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Message ExAgent")).not.toBeInTheDocument();
  });

  it("compares a forked session from an inactive expanded project", async () => {
    const user = userEvent.setup();
    vi.spyOn(exagentClient, "reindexProject").mockImplementation(async (projectId) => {
      if (projectId === "project-beta") {
        return [
          threadRecord({
            id: "session-beta-parent",
            project_id: "project-beta",
            fallback_title: "Beta parent",
            created_at: 1,
            updated_at: 1,
          }),
          threadRecord({
            id: "session-beta-child",
            project_id: "project-beta",
            fallback_title: "Beta child",
            created_at: 2,
            updated_at: 2,
            fork_parent_thread_id: "session-beta-parent",
            fork_point_turn_id: "turn-1",
          }),
        ];
      }
      return [];
    });
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-beta-child",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [],
      },
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(null);
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-beta-child",
      events: [],
    });
    vi.spyOn(exagentClient, "readThread").mockImplementation(async (_projectId, threadId) => {
      if (threadId === "session-beta-parent") {
        return {
          thread: {
            id: "session-beta-parent",
            status: "idle",
            goal_mode: "standard",
            active_turn: null,
            turns: [
              {
                id: "turn-1",
                status: "completed",
                items: [{ type: "user_message", text: "Shared beta prompt" }],
              },
              {
                id: "turn-parent",
                status: "completed",
                items: [{ type: "assistant_message", text: "Beta parent only" }],
              },
            ],
          },
        };
      }
      return {
        thread: {
          id: "session-beta-child",
          status: "idle",
          goal_mode: "standard",
          active_turn: null,
          turns: [
            {
              id: "turn-1",
              status: "completed",
              items: [{ type: "user_message", text: "Shared beta prompt" }],
            },
            {
              id: "turn-child",
              status: "completed",
              items: [{ type: "assistant_message", text: "Beta child only" }],
            },
          ],
        },
      };
    });
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-alpha",
          name: "Alpha",
          path: "/tmp/alpha",
          active: true,
        },
        {
          id: "project-beta",
          name: "Beta",
          path: "/tmp/beta",
          active: false,
        },
      ],
      sessions: [
        {
          id: "session-alpha",
          projectId: "project-alpha",
          title: "Alpha session",
          updatedAt: "now",
          status: "idle",
        },
      ],
      activeProjectId: "project-alpha",
      activeSessionId: "session-alpha",
    });

    render(<App />);
    await user.click(screen.getByRole("button", { name: /^Beta$/ }));
    expect(await screen.findByText("Beta child")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Session actions for Beta child" }));
    await user.click(screen.getByRole("menuitem", { name: "Compare with parent" }));

    expect(await screen.findByLabelText("Parent branch transcript")).toBeInTheDocument();
    expect(screen.getByLabelText("Fork branch transcript")).toBeInTheDocument();
    expect(screen.getByText("Beta parent only")).toBeInTheDocument();
    expect(screen.getByText("Beta child only")).toBeInTheDocument();
    expect(useWorkbenchStore.getState().activeProjectId).toBe("project-beta");
  });

  it.each([
    ["child", "session-child", "Archive Child fork"],
    ["parent", "session-parent", "Archive Parent session"],
  ] as const)("clears branch compare when archiving the compared %s", async (_branch, sessionId, archiveLabel) => {
    const user = userEvent.setup();
    const archiveThread = vi.spyOn(exagentClient, "archiveThread").mockResolvedValue(undefined);
    vi.spyOn(exagentClient, "readThread").mockImplementation(async (_projectId, threadId) => {
      if (threadId === "session-parent") {
        return {
          thread: {
            id: "session-parent",
            status: "idle",
            goal_mode: "standard",
            active_turn: null,
            turns: [
              {
                id: "turn-1",
                status: "completed",
                items: [{ type: "user_message", text: "Shared prompt" }],
              },
              {
                id: "turn-parent",
                status: "completed",
                items: [{ type: "assistant_message", text: "Parent-only follow-up" }],
              },
            ],
          },
        };
      }
      return {
        thread: {
          id: "session-child",
          status: "idle",
          goal_mode: "standard",
          active_turn: null,
          turns: [
            {
              id: "turn-1",
              status: "completed",
              items: [{ type: "user_message", text: "Shared prompt" }],
            },
            {
              id: "turn-child",
              status: "completed",
              items: [{ type: "assistant_message", text: "Fork-only follow-up" }],
            },
          ],
        },
      };
    });
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Users/enxiang/dev/ExAgent",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-parent",
          projectId: "project-exagent",
          title: "Parent session",
          updatedAt: "now",
          status: "idle",
          createdAt: 1,
        },
        {
          id: "session-child",
          projectId: "project-exagent",
          title: "Child fork",
          updatedAt: "now",
          status: "idle",
          createdAt: 2,
          forkParentThreadId: "session-parent",
          forkPointTurnId: "turn-1",
        },
      ],
      activeProjectId: "project-exagent",
      activeSessionId: "session-child",
      transcript: [
        {
          id: "child-current",
          role: "assistant",
          body: "Fork-only follow-up",
          timestamp: "history",
          threadId: "session-child",
          turnId: "turn-child",
          turnStatus: "completed",
        },
      ],
    });

    render(<App />);
    await user.click(screen.getByRole("button", { name: "Session actions for Child fork" }));
    await user.click(screen.getByRole("menuitem", { name: "Compare with parent" }));
    expect(await screen.findByLabelText("Parent branch transcript")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: archiveLabel }));

    expect(archiveThread).toHaveBeenCalledWith(sessionId);
    await waitFor(() => {
      expect(screen.queryByLabelText("Parent branch transcript")).not.toBeInTheDocument();
    });
    expect(useWorkbenchStore.getState().compareThreadId).toBeNull();
    expect(useWorkbenchStore.getState().compareView).toBeNull();
  });

  it("does not reopen branch compare from stale pending reads after archiving a compared thread", async () => {
    const user = userEvent.setup();
    const parentRead = createDeferred<Awaited<ReturnType<typeof exagentClient.readThread>>>();
    const childRead = createDeferred<Awaited<ReturnType<typeof exagentClient.readThread>>>();
    vi.spyOn(exagentClient, "archiveThread").mockResolvedValue(undefined);
    vi.spyOn(exagentClient, "readThread").mockImplementation((_projectId, threadId) => {
      return threadId === "session-parent" ? parentRead.promise : childRead.promise;
    });
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Users/enxiang/dev/ExAgent",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-parent",
          projectId: "project-exagent",
          title: "Parent session",
          updatedAt: "now",
          status: "idle",
          createdAt: 1,
        },
        {
          id: "session-child",
          projectId: "project-exagent",
          title: "Child fork",
          updatedAt: "now",
          status: "idle",
          createdAt: 2,
          forkParentThreadId: "session-parent",
          forkPointTurnId: "turn-1",
        },
      ],
      activeProjectId: "project-exagent",
      activeSessionId: "session-child",
      transcript: [
        {
          id: "child-current",
          role: "assistant",
          body: "Fork-only follow-up",
          timestamp: "history",
          threadId: "session-child",
          turnId: "turn-child",
          turnStatus: "completed",
        },
      ],
    });

    render(<App />);
    await user.click(screen.getByRole("button", { name: "Session actions for Child fork" }));
    await user.click(screen.getByRole("menuitem", { name: "Compare with parent" }));
    expect(await screen.findByLabelText("Parent branch transcript")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Archive Child fork" }));
    await waitFor(() => {
      expect(screen.queryByLabelText("Parent branch transcript")).not.toBeInTheDocument();
    });

    await act(async () => {
      parentRead.resolve({
        thread: {
          id: "session-parent",
          status: "idle",
          goal_mode: "standard",
          active_turn: null,
          turns: [
            {
              id: "turn-1",
              status: "completed",
              items: [{ type: "user_message", text: "Shared prompt" }],
            },
            {
              id: "turn-parent",
              status: "completed",
              items: [{ type: "assistant_message", text: "Late parent compare" }],
            },
          ],
        },
      });
      childRead.resolve({
        thread: {
          id: "session-child",
          status: "idle",
          goal_mode: "standard",
          active_turn: null,
          turns: [
            {
              id: "turn-1",
              status: "completed",
              items: [{ type: "user_message", text: "Shared prompt" }],
            },
            {
              id: "turn-child",
              status: "completed",
              items: [{ type: "assistant_message", text: "Late child compare" }],
            },
          ],
        },
      });
      await Promise.resolve();
    });

    expect(screen.queryByLabelText("Parent branch transcript")).not.toBeInTheDocument();
    expect(screen.queryByText("Late parent compare")).not.toBeInTheDocument();
    expect(screen.queryByText("Late child compare")).not.toBeInTheDocument();
    expect(useWorkbenchStore.getState().compareThreadId).toBeNull();
    expect(useWorkbenchStore.getState().compareView).toBeNull();
  });

  it("closes branch compare from the visible close control", async () => {
    const user = userEvent.setup();
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Users/enxiang/dev/ExAgent",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-parent",
          projectId: "project-exagent",
          title: "Parent session",
          updatedAt: "now",
          status: "idle",
          createdAt: 1,
        },
        {
          id: "session-child",
          projectId: "project-exagent",
          title: "Child fork",
          updatedAt: "now",
          status: "idle",
          createdAt: 2,
          forkParentThreadId: "session-parent",
          forkPointTurnId: "turn-1",
        },
      ],
      activeProjectId: "project-exagent",
      activeSessionId: "session-child",
      transcript: [
        {
          id: "child-current",
          role: "assistant",
          body: "Fork-only follow-up",
          timestamp: "history",
          threadId: "session-child",
          turnId: "turn-child",
          turnStatus: "completed",
        },
      ],
      compareThreadId: "session-child",
      compareView: {
        parentThreadId: "session-parent",
        childThreadId: "session-child",
        parentTitle: "Parent session",
        childTitle: "Child fork",
        parentTranscript: [
          {
            id: "parent-message",
            role: "assistant",
            body: "Parent-only follow-up",
            timestamp: "history",
            threadId: "session-parent",
            turnId: "turn-parent",
            turnStatus: "completed",
          },
        ],
        childTranscript: [
          {
            id: "child-message",
            role: "assistant",
            body: "Fork-only follow-up",
            timestamp: "history",
            threadId: "session-child",
            turnId: "turn-child",
            turnStatus: "completed",
          },
        ],
        sharedTurnCount: 1,
        forkPointTurnId: "turn-1",
        loading: false,
        error: null,
      },
    });

    render(<App />);
    expect(screen.getByLabelText("Parent branch transcript")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Close branch compare" }));

    expect(screen.queryByLabelText("Parent branch transcript")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Message ExAgent")).toBeInTheDocument();
    expect(useWorkbenchStore.getState().compareView).toBeNull();
  });

  it("preserves incoming fork session order while nesting children", () => {
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Users/enxiang/dev/ExAgent",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-recent-root",
          projectId: "project-exagent",
          title: "Recent root",
          updatedAt: "now",
          status: "idle",
          createdAt: 300,
        },
        {
          id: "session-child-first",
          projectId: "project-exagent",
          title: "Child first",
          updatedAt: "now",
          status: "idle",
          createdAt: 20,
          forkParentThreadId: "session-parent-order",
          forkPointTurnId: "turn-1",
        },
        {
          id: "session-parent-order",
          projectId: "project-exagent",
          title: "Parent in incoming order",
          updatedAt: "now",
          status: "idle",
          createdAt: 100,
        },
        {
          id: "session-child-second",
          projectId: "project-exagent",
          title: "Child second",
          updatedAt: "now",
          status: "idle",
          createdAt: 10,
          forkParentThreadId: "session-parent-order",
          forkPointTurnId: "turn-2",
        },
        {
          id: "session-older-root",
          projectId: "project-exagent",
          title: "Older root",
          updatedAt: "now",
          status: "idle",
          createdAt: 50,
        },
      ],
      activeProjectId: "project-exagent",
      activeSessionId: "session-recent-root",
    });

    render(<App />);

    const sidebar = screen.getByRole("complementary", {
      name: "Projects and sessions",
    });
    const recentRoot = within(sidebar).getByText("Recent root");
    const parent = within(sidebar).getByText("Parent in incoming order");
    const childFirst = within(sidebar).getByText("Child first");
    const childSecond = within(sidebar).getByText("Child second");
    const olderRoot = within(sidebar).getByText("Older root");

    expect(recentRoot.compareDocumentPosition(parent) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(parent.compareDocumentPosition(childFirst) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(childFirst.compareDocumentPosition(childSecond) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(childSecond.compareDocumentPosition(olderRoot) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(childFirst.closest("[data-session-branch-group]")).toContainElement(childSecond);
  });

  it("shows a focused draft-session state before a real session exists", async () => {
    vi.spyOn(exagentClient, "getWorkbenchSnapshot").mockResolvedValue({
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Volumes/EXEXEX/ExAgent",
          active: true,
        },
      ],
      sessions: [],
      activeProjectId: "project-exagent",
      activeSessionId: null,
      transcript: [],
      events: [],
      changedFiles: [],
      cwd: "/Volumes/EXEXEX/ExAgent",
      policy: "local",
      tokenUsage: {
        input: 0,
        output: 0,
        limit: 1,
      },
      tokenUsageByThreadId: {},
      runtimeSettings: null,
      selectedModel: null,
      selectedThinkingMode: null,
    });
    vi.spyOn(exagentClient, "getRuntimeSettings").mockResolvedValue({
      default_model: "gpt-5.5",
      default_thinking_mode: "medium",
      presets: [],
      mcp_servers: [],
      skill_roots: [],
    });
    vi.spyOn(exagentClient, "getProviderSettings").mockResolvedValue({
      providers: [],
      active_provider_id: "openai",
      config: {
        provider_id: "openai",
        base_url: "https://api.openai.com/v1",
        model: "gpt-5.5",
        has_api_key: false,
        credential_source: "none",
        auth_required: true,
      },
      connected_provider: null,
      last_connection: null,
      configured_providers: [],
      model_options: [],
    });

    render(<App />);

    expect(
      await screen.findByText("What should we build in ExAgent?"),
    ).toBeInTheDocument();
    expect(screen.getByText("Build a feature")).toBeInTheDocument();
    expect(screen.queryByText("Start a session")).not.toBeInTheDocument();
  });

  it("prompts provider configuration instead of showing placeholder models when no provider is configured", async () => {
    const user = userEvent.setup();
    vi.spyOn(exagentClient, "getWorkbenchSnapshot").mockResolvedValue({
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Volumes/EXEXEX/ExAgent",
          active: true,
        },
      ],
      sessions: [],
      activeProjectId: "project-exagent",
      activeSessionId: null,
      transcript: [],
      events: [],
      changedFiles: [],
      cwd: "/Volumes/EXEXEX/ExAgent",
      policy: "local",
      tokenUsage: {
        input: 0,
        output: 0,
        limit: 1,
      },
      tokenUsageByThreadId: {},
      runtimeSettings: null,
      selectedModel: null,
      selectedThinkingMode: null,
    });
    vi.spyOn(exagentClient, "getRuntimeSettings").mockResolvedValue({
      default_model: "gpt-5.5",
      default_thinking_mode: "medium",
      presets: [],
      mcp_servers: [],
      skill_roots: [],
    });
    vi.spyOn(exagentClient, "getProviderSettings").mockResolvedValue({
      providers: [deepSeekProvider],
      active_provider_id: "deepseek",
      config: {
        provider_id: "deepseek",
        base_url: "https://api.deepseek.com",
        model: "deepseek-v4-flash",
        has_api_key: false,
        credential_source: "none",
        auth_required: true,
      },
      connected_provider: null,
      last_connection: null,
      configured_providers: [],
      model_options: [],
    });

    render(<App />);

    await screen.findByText("What should we build in ExAgent?");
    const modelButton = screen.getByRole("button", { name: "Composer model" });

    expect(modelButton).toHaveTextContent("Configure provider");
    expect(modelButton).not.toHaveTextContent("deepseek-v4-flash");

    await user.click(modelButton);

    expect(
      screen.getByText("Configure a provider to choose a model."),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("menuitemradio", { name: /deepseek-v4-flash/i }),
    ).not.toBeInTheDocument();
  });

  it("loads the selected model from provider settings instead of runtime defaults", async () => {
    vi.spyOn(exagentClient, "getWorkbenchSnapshot").mockResolvedValue({
      projects: [],
      sessions: [],
      activeProjectId: null,
      activeSessionId: null,
      transcript: [],
      events: [],
      changedFiles: [],
      cwd: "No project selected",
      policy: "local",
      tokenUsage: {
        input: 0,
        output: 0,
        limit: 1,
      },
      tokenUsageByThreadId: {},
      runtimeSettings: null,
      selectedModel: null,
      selectedThinkingMode: null,
    });
    vi.spyOn(exagentClient, "getRuntimeSettings").mockResolvedValue({
      default_model: "runtime-default",
      default_thinking_mode: "high",
      presets: [],
      mcp_servers: [],
      skill_roots: [],
    });
    vi.spyOn(exagentClient, "getProviderSettings").mockResolvedValue({
      providers: [],
      active_provider_id: "openai",
      config: {
        provider_id: "openai_compatible",
        base_url: "http://127.0.0.1:11434/v1",
        model: "configured-model",
        has_api_key: false,
        credential_source: "none",
        auth_required: false,
      },
      connected_provider: null,
      last_connection: null,
      configured_providers: [],
      model_options: [
        {
          provider_id: "openai_compatible",
          id: "configured-model",
          display_name: "configured-model",
          context_window: null,
          supports_tools: true,
          capabilities: {
            supports_tools: true,
            thinking: {
              supported: false,
              modes: [],
            },
          },
        },
      ],
    });

    await useWorkbenchStore.getState().loadWorkbench();

    expect(useWorkbenchStore.getState().activeProviderId).toBe(
      "openai_compatible",
    );
    expect(useWorkbenchStore.getState().selectedModel).toEqual({
      provider_id: "openai_compatible",
      model_id: "configured-model",
    });
    expect(useWorkbenchStore.getState().selectedThinkingMode).toBeNull();
  });

  it("opens settings to the providers tab from the lower sidebar", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    await userEvent.click(
      screen.getByRole("button", { name: "Open settings" }),
    );

    expect(
      screen.getByRole("heading", { name: "Settings" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Providers" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(
      screen.getByRole("heading", { name: "Popular" }),
    ).toBeInTheDocument();
    expect(screen.getByTestId("provider-popular-list")).toHaveClass(
      "space-y-1",
    );
    expect(
      screen.getByText("Use ChatGPT Pro/Plus or an API key"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Configure OpenRouter" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Configure Vercel AI Gateway" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Configure DeepSeek" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Configure Kimi" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Configure GLM" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Configure OpenAI" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Configure Google" }),
    ).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "Configure Anthropic" }),
    ).toBeEnabled();
    const providerButtons = within(
      screen.getByTestId("provider-popular-list"),
    ).getAllByRole("button");
    expect(
      providerButtons
        .slice(0, 4)
        .map((button) => button.getAttribute("aria-label")),
    ).toEqual([
      "Configure OpenAI",
      "Configure Anthropic",
      "Configure Google",
      "Configure DeepSeek",
    ]);
    expect(
      within(
        screen.getByRole("button", { name: "Configure OpenAI Compatible" }),
      ).queryByText("Recommended"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByText("Use GitHub Copilot with device OAuth"),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("Copilot account support is planned"),
    ).not.toBeInTheDocument();
  });

  it("supports keyboard navigation across settings tabs", async () => {
    const user = userEvent.setup();
    render(<App />);

    await screen.findByText("Session restored");
    await user.click(screen.getByRole("button", { name: "Open settings" }));

    const providersTab = screen.getByRole("tab", { name: "Providers" });
    providersTab.focus();
    await user.keyboard("{ArrowRight}");

    await waitFor(() => {
      expect(screen.getByRole("tab", { name: "MCP" })).toHaveAttribute(
        "aria-selected",
        "true",
      );
    });

    await user.keyboard("{End}");

    await waitFor(() => {
      expect(screen.getByRole("tab", { name: "Archive" })).toHaveAttribute(
        "aria-selected",
        "true",
      );
    });
  });

  it("tests provider connections from settings before saving", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    await userEvent.click(
      screen.getByRole("button", { name: "Open settings" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Configure OpenAI" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Test connection" }),
    );

    expect(
      await screen.findByText("Connection succeeded."),
    ).toBeInTheDocument();
  });

  it("opens a provider-specific connection page from Configure", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    await userEvent.click(
      screen.getByRole("button", { name: "Open settings" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Configure OpenAI Compatible" }),
    );

    const dialog = screen.getByRole("dialog", {
      name: "Connect OpenAI Compatible",
    });
    expect(dialog).toHaveClass("max-w-[920px]");
    expect(within(dialog).getByTestId("provider-connection-body")).toHaveClass(
      "max-w-[720px]",
    );
    expect(
      within(dialog).getByRole("button", { name: "Back to providers" }),
    ).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Base URL")).toHaveValue(
      "http://127.0.0.1:11434/v1",
    );
  });

  it("opens OpenRouter as an OpenAI-compatible preset", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    await userEvent.click(
      screen.getByRole("button", { name: "Open settings" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Configure OpenRouter" }),
    );

    const dialog = screen.getByRole("dialog", { name: "Connect OpenRouter" });
    expect(within(dialog).getByLabelText("Base URL")).toHaveValue(
      "https://openrouter.ai/api/v1",
    );
    expect(within(dialog).getByLabelText("Model")).toHaveValue(
      "openrouter/auto",
    );
  });

  it("shows OpenAI login method choices before API key fields", async () => {
    const user = userEvent.setup();
    render(<App />);

    await screen.findByText("Session restored");
    await user.click(screen.getByRole("button", { name: "Open settings" }));
    await user.click(screen.getByRole("button", { name: "Configure OpenAI" }));

    expect(
      screen.getByRole("dialog", { name: "Connect OpenAI" }),
    ).toBeInTheDocument();
    const authModeGroup = screen.getByRole("radiogroup", {
      name: "OpenAI auth mode",
    });
    expect(
      within(authModeGroup).getByRole("radio", {
        name: "ChatGPT Pro/Plus (headless)",
      }),
    ).toHaveAttribute("aria-checked", "false");

    const apiKeyMode = within(authModeGroup).getByRole("radio", {
      name: "API key",
    });
    expect(apiKeyMode).toHaveAttribute("aria-checked", "true");

    apiKeyMode.focus();
    await user.keyboard("{ArrowLeft}");

    await waitFor(() => {
      expect(
        within(authModeGroup).getByRole("radio", {
          name: "ChatGPT Pro/Plus (headless)",
        }),
      ).toHaveAttribute("aria-checked", "true");
    });
  });

  it("runs OpenAI headless OAuth device login from settings", async () => {
    const user = userEvent.setup();
    const openExternalUrl = vi
      .spyOn(exagentClient, "openExternalUrl")
      .mockResolvedValue(undefined);
    vi.spyOn(exagentClient, "startChatGptOAuthDevice").mockResolvedValue({
      device_auth_id: "device-1",
      user_code: "ABCD-EFGH",
      verification_uri: "https://auth.openai.com/codex/device",
      expires_in: 900,
      interval: 1,
    });
    vi.spyOn(exagentClient, "completeChatGptOAuthDevice").mockResolvedValue(
      oauthProviderSettings("openai", "OpenAI"),
    );

    render(<App />);

    await screen.findByText("Session restored");
    await user.click(screen.getByRole("button", { name: "Open settings" }));
    await user.click(screen.getByRole("button", { name: "Configure OpenAI" }));
    await user.click(
      screen.getByRole("radio", { name: "ChatGPT Pro/Plus (headless)" }),
    );
    await user.click(screen.getByRole("button", { name: "Start OAuth login" }));

    expect(await screen.findByText("ABCD-EFGH")).toBeInTheDocument();
    expect(openExternalUrl).toHaveBeenCalledWith(
      "https://auth.openai.com/codex/device",
    );

    await user.click(
      screen.getByRole("button", { name: "Complete OAuth login" }),
    );

    expect(exagentClient.completeChatGptOAuthDevice).toHaveBeenCalledWith({
      device_auth_id: "device-1",
      user_code: "ABCD-EFGH",
      verification_uri: "https://auth.openai.com/codex/device",
      expires_in: 900,
      interval: 1,
    });
  });

  it("shows GitHub Copilot deployment choices", async () => {
    const user = userEvent.setup();
    const openExternalUrl = vi
      .spyOn(exagentClient, "openExternalUrl")
      .mockResolvedValue(undefined);
    vi.spyOn(exagentClient, "startGitHubCopilotOAuthDevice").mockResolvedValue({
      device_code: "device-code-1",
      user_code: "WXYZ-1234",
      verification_uri: "https://github.com/login/device",
      expires_in: 900,
      interval: 1,
    });
    vi.spyOn(
      exagentClient,
      "completeGitHubCopilotOAuthDevice",
    ).mockResolvedValue(
      oauthProviderSettings("github_copilot", "GitHub Copilot"),
    );

    render(<App />);

    await screen.findByText("Session restored");
    await user.click(screen.getByRole("button", { name: "Open settings" }));
    await user.click(
      screen.getByRole("button", { name: "Configure GitHub Copilot" }),
    );

    expect(
      screen.getByRole("dialog", { name: "Connect GitHub Copilot" }),
    ).toBeInTheDocument();
    expect(screen.getByText("GitHub.com")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Start OAuth login" }));

    expect(await screen.findByText("WXYZ-1234")).toBeInTheDocument();
    expect(openExternalUrl).toHaveBeenCalledWith(
      "https://github.com/login/device",
    );

    await user.click(
      screen.getByRole("button", { name: "Complete OAuth login" }),
    );
    expect(exagentClient.completeGitHubCopilotOAuthDevice).toHaveBeenCalled();
  });

  it("opens Anthropic as a supported API key provider", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    await userEvent.click(
      screen.getByRole("button", { name: "Open settings" }),
    );
    expect(
      screen.getByRole("button", { name: "Configure Anthropic" }),
    ).toBeEnabled();
    await userEvent.click(
      screen.getByRole("button", { name: "Configure Anthropic" }),
    );

    const dialog = screen.getByRole("dialog", { name: "Connect Anthropic" });
    expect(within(dialog).getByLabelText("Anthropic API key")).toBeEnabled();
    expect(
      within(dialog).getByRole("button", { name: "Save provider" }),
    ).toBeEnabled();
    expect(
      within(dialog).queryByText("Anthropic Messages adapter is planned."),
    ).not.toBeInTheDocument();
  });

  it("opens Google and OpenAI-compatible vendor providers", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    await userEvent.click(
      screen.getByRole("button", { name: "Open settings" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Configure Google" }),
    );

    let dialog = screen.getByRole("dialog", { name: "Connect Google" });
    expect(within(dialog).getByLabelText("Google API key")).toBeEnabled();
    expect(within(dialog).getByLabelText("Model")).toHaveValue(
      "gemini-3-pro-preview",
    );
    expect(
      within(dialog).getByRole("button", { name: "Save provider" }),
    ).toBeEnabled();

    await userEvent.click(
      within(dialog).getByRole("button", { name: "Back to providers" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Configure DeepSeek" }),
    );
    dialog = screen.getByRole("dialog", { name: "Connect DeepSeek" });
    expect(within(dialog).getByLabelText("DeepSeek API key")).toBeEnabled();
    expect(
      within(dialog).queryByLabelText("API key credential"),
    ).not.toBeInTheDocument();
    expect(
      within(dialog).queryByRole("button", { name: "Add key" }),
    ).not.toBeInTheDocument();
    expect(within(dialog).getByLabelText("Base URL")).toHaveValue(
      "https://api.deepseek.com",
    );
    expect(within(dialog).getByLabelText("Model")).toHaveValue(
      "deepseek-v4-flash",
    );

    await userEvent.click(
      within(dialog).getByRole("button", { name: "Back to providers" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Configure Kimi" }),
    );
    dialog = screen.getByRole("dialog", { name: "Connect Kimi" });
    expect(within(dialog).getByLabelText("Base URL")).toHaveValue(
      "https://api.moonshot.ai/v1",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Base URL presets" }),
    );
    await userEvent.click(
      screen.getByRole("menuitem", { name: /Mainland China/ }),
    );
    expect(within(dialog).getByLabelText("Base URL")).toHaveValue(
      "https://api.moonshot.cn/v1",
    );
    await userEvent.clear(within(dialog).getByLabelText("Base URL"));
    await userEvent.type(
      within(dialog).getByLabelText("Base URL"),
      "https://moonshot.example/v1",
    );
    expect(within(dialog).getByLabelText("Base URL")).toHaveValue(
      "https://moonshot.example/v1",
    );
    expect(within(dialog).getByLabelText("Model")).toHaveValue("kimi-k2.6");

    await userEvent.click(
      within(dialog).getByRole("button", { name: "Back to providers" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Configure GLM" }),
    );
    dialog = screen.getByRole("dialog", { name: "Connect GLM" });
    expect(within(dialog).getByLabelText("Base URL")).toHaveValue(
      "https://open.bigmodel.cn/api/paas/v4",
    );
    expect(within(dialog).getByLabelText("Model")).toHaveValue("glm-5.1");
  });

  it("uses a single API key field without credential profile controls", async () => {
    const user = userEvent.setup();
    const saveProviderSettings = vi.spyOn(
      exagentClient,
      "saveProviderSettings",
    );
    render(<App />);

    await screen.findByText("Session restored");
    await user.click(screen.getByRole("button", { name: "Open settings" }));
    await user.click(
      screen.getByRole("button", { name: "Configure DeepSeek" }),
    );
    const dialog = screen.getByRole("dialog", { name: "Connect DeepSeek" });

    expect(within(dialog).queryByText("Credential")).not.toBeInTheDocument();
    expect(
      within(dialog).queryByLabelText("API key credential"),
    ).not.toBeInTheDocument();
    expect(
      within(dialog).queryByRole("button", { name: "Add key" }),
    ).not.toBeInTheDocument();

    await user.type(
      within(dialog).getByLabelText("DeepSeek API key"),
      "sk-deepseek",
    );
    await user.click(
      within(dialog).getByRole("button", { name: "Save provider" }),
    );

    expect(saveProviderSettings).toHaveBeenCalledWith(
      expect.objectContaining({
        providerId: "deepseek",
        apiKey: "sk-deepseek",
      }),
    );
    expect(saveProviderSettings).toHaveBeenCalledWith(
      expect.not.objectContaining({
        credentialId: expect.anything(),
        createCredential: expect.anything(),
      }),
    );
  });

  it("discovers models on save and renders configured provider groups in the composer", async () => {
    const user = userEvent.setup();
    vi.spyOn(exagentClient, "listProviderModels").mockResolvedValue({
      status: "success",
      message: "Model discovery succeeded.",
      models: [
        {
          provider_id: "deepseek",
          id: "deepseek-chat",
          display_name: "deepseek-chat",
          context_window: null,
          supports_tools: true,
          capabilities: {
            supports_tools: true,
            thinking: {
              supported: false,
              modes: [],
            },
          },
        },
        {
          provider_id: "deepseek",
          id: "deepseek-reasoner",
          display_name: "deepseek-reasoner",
          context_window: null,
          supports_tools: true,
          capabilities: {
            supports_tools: true,
            thinking: {
              supported: true,
              modes: ["off", "high"],
            },
          },
        },
      ],
    });
    const saveProviderSettings = vi
      .spyOn(exagentClient, "saveProviderSettings")
      .mockResolvedValue(
        deepSeekProviderSettings({
          providers: [
            {
              id: "openai",
              name: "OpenAI",
              description: "Use ChatGPT Pro/Plus or an API key",
              recommended: true,
              supported: true,
              auth_mode: "api_key_required",
              protocol: "openai_chat_completions",
              default_base_url: "https://api.openai.com/v1",
              default_model: "gpt-5.5",
              supports_model_discovery: true,
              supports_tools: true,
              unsupported_reason: null,
            },
            deepSeekProvider,
          ],
          configured_providers: [
            {
              provider_id: "openai",
              base_url: "https://api.openai.com/v1",
              model: "gpt-5.5",
              has_api_key: true,
              credential_source: "keychain",
              auth_required: true,
            },
            {
              provider_id: "deepseek",
              base_url: "https://api.deepseek.com",
              model: "deepseek-v4-flash",
              has_api_key: true,
              credential_source: "keychain",
              auth_required: true,
            },
          ],
          model_options: [
            {
              provider_id: "openai",
              id: "gpt-5.5",
              display_name: "gpt-5.5",
              context_window: null,
              supports_tools: true,
              capabilities: {
                supports_tools: true,
                thinking: {
                  supported: true,
                  modes: ["off", "high"],
                },
              },
            },
            {
              provider_id: "deepseek",
              id: "deepseek-chat",
              display_name: "deepseek-chat",
              context_window: null,
              supports_tools: true,
              capabilities: {
                supports_tools: true,
                thinking: {
                  supported: false,
                  modes: [],
                },
              },
            },
            {
              provider_id: "deepseek",
              id: "deepseek-reasoner",
              display_name: "deepseek-reasoner",
              context_window: null,
              supports_tools: true,
              capabilities: {
                supports_tools: true,
                thinking: {
                  supported: true,
                  modes: ["off", "high"],
                },
              },
            },
          ],
        }),
      );

    render(<App />);

    await screen.findByText("Session restored");
    await user.click(screen.getByRole("button", { name: "Open settings" }));
    await user.click(
      screen.getByRole("button", { name: "Configure DeepSeek" }),
    );
    const dialog = screen.getByRole("dialog", { name: "Connect DeepSeek" });
    await user.type(
      within(dialog).getByLabelText("DeepSeek API key"),
      "sk-deepseek",
    );
    await user.click(
      within(dialog).getByRole("button", { name: "Save provider" }),
    );

    await waitFor(() => {
      expect(saveProviderSettings).toHaveBeenCalledWith(
        expect.objectContaining({
          providerId: "deepseek",
          modelOptions: expect.arrayContaining([
            expect.objectContaining({
              provider_id: "deepseek",
              id: "deepseek-chat",
            }),
            expect.objectContaining({
              provider_id: "deepseek",
              id: "deepseek-reasoner",
            }),
          ]),
        }),
      );
    });

    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "Composer model" }));

    expect(screen.getByText("OpenAI")).toBeInTheDocument();
    expect(screen.getByText("DeepSeek")).toBeInTheDocument();
    expect(
      screen.getAllByRole("menuitemradio", { name: /gpt-5.5/i }).length,
    ).toBeGreaterThan(0);
    expect(
      screen.getByRole("menuitemradio", { name: /deepseek-chat/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("menuitemradio", { name: /deepseek-reasoner/i }),
    ).toBeInTheDocument();
  });

  it("discovers models from provider settings and keeps manual entry available", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    await userEvent.click(
      screen.getByRole("button", { name: "Open settings" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Configure OpenAI Compatible" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Discover models" }),
    );

    expect(
      await screen.findByRole("button", { name: "Use gpt-4.1-mini" }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("Model")).toBeEnabled();
    await userEvent.click(
      screen.getByRole("button", { name: "Use local-coder" }),
    );
    expect(screen.getByLabelText("Model")).toHaveValue("local-coder");
    expect(
      screen.getByRole("button", { name: "Use gpt-4.1-mini" }),
    ).toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: "Save provider" }),
    );
    expect(
      useWorkbenchStore
        .getState()
        .providerSettings?.model_options.some(
          (model) =>
            model.provider_id === "openai_compatible" &&
            model.id === "local-coder",
        ),
    ).toBe(true);
    expect(
      useWorkbenchStore
        .getState()
        .providerSettings?.model_options.some(
          (model) =>
            model.provider_id === "openai_compatible" &&
            model.id === "gpt-4.1-mini",
        ),
    ).toBe(true);
  });

  it("shows a searchable model picker and renders thinking only for capable models", async () => {
    const user = userEvent.setup();
    render(<App />);

    await screen.findByText("Session restored");

    const modelButton = screen.getByRole("button", { name: "Composer model" });
    expect(modelButton).toHaveTextContent("gpt-5.5");
    expect(
      screen.getByRole("button", { name: "Thinking mode" }),
    ).toBeInTheDocument();

    await user.click(modelButton);
    expect(screen.getByLabelText("Search models")).toBeInTheDocument();
    expect(screen.getByText("OpenAI")).toBeInTheDocument();
    expect(screen.queryByText("OpenAI Compatible")).not.toBeInTheDocument();
    await user.click(screen.getByLabelText("Search models"));
    await user.keyboard("{Escape}");
    expect(screen.queryByLabelText("Search models")).not.toBeInTheDocument();

    act(() => {
      const currentProviderSettings =
        useWorkbenchStore.getState().providerSettings;
      useWorkbenchStore.setState({
        providerSettings: currentProviderSettings
          ? {
              ...currentProviderSettings,
              model_options: [
                ...currentProviderSettings.model_options,
                {
                  provider_id: "openai_compatible",
                  id: "local-model",
                  display_name: "local-model",
                  context_window: null,
                  supports_tools: true,
                  capabilities: {
                    supports_tools: false,
                    thinking: {
                      supported: false,
                      modes: [],
                    },
                  },
                },
              ],
            }
          : currentProviderSettings,
      });
    });

    await user.click(modelButton);
    await user.type(screen.getByLabelText("Search models"), "local");
    await user.click(
      screen.getByRole("menuitemradio", { name: /local-model/i }),
    );
    expect(modelButton).toHaveTextContent("local-model");
    expect(
      screen.queryByRole("button", { name: "Thinking mode" }),
    ).not.toBeInTheDocument();

    act(() => {
      const currentProviderSettings =
        useWorkbenchStore.getState().providerSettings;
      useWorkbenchStore.setState({
        providerSettings: currentProviderSettings
          ? {
              ...currentProviderSettings,
              model_options: [
                ...currentProviderSettings.model_options,
                {
                  provider_id: "openai",
                  id: "gpt-5",
                  display_name: "gpt-5",
                  context_window: 400000,
                  supports_tools: true,
                  capabilities: {
                    supports_tools: true,
                    thinking: {
                      supported: true,
                      modes: ["off", "high"],
                    },
                  },
                },
              ],
            }
          : currentProviderSettings,
        selectedModel: {
          provider_id: "openai",
          model_id: "gpt-5",
        },
        selectedThinkingMode: "auto",
      });
    });

    expect(
      screen.getByRole("button", { name: "Thinking mode" }),
    ).toHaveTextContent("Default");
    await user.click(screen.getByRole("button", { name: "Thinking mode" }));
    expect(
      screen.getByRole("menuitemradio", { name: "Thinking default" }),
    ).toHaveAttribute("aria-checked", "true");
    expect(
      screen.getByRole("menuitemradio", { name: "Thinking off" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("menuitemradio", { name: "Thinking high" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("menuitemradio", { name: "Thinking low" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("menuitemradio", { name: "Thinking xhigh" }),
    ).not.toBeInTheDocument();

    act(() => {
      const currentProviderSettings =
        useWorkbenchStore.getState().providerSettings;
      useWorkbenchStore.setState({
        providerSettings: currentProviderSettings
          ? {
              ...currentProviderSettings,
              model_options: currentProviderSettings.model_options.map(
                (model) =>
                  model.provider_id === "openai" && model.id === "gpt-5"
                    ? {
                        ...model,
                        capabilities: {
                          ...model.capabilities,
                          thinking: {
                            supported: true,
                            modes: ["off", "high", "x_high"],
                          },
                        },
                      }
                    : model,
              ),
            }
          : currentProviderSettings,
      });
    });

    await user.click(
      screen.getByRole("menuitemradio", { name: "Thinking xhigh" }),
    );
    expect(useWorkbenchStore.getState().selectedThinkingMode).toBe("x_high");
  });

  it("opens localized composer context actions from the plus menu", async () => {
    const user = userEvent.setup();
    const setThreadGoal = vi
      .spyOn(exagentClient, "setThreadGoal")
      .mockResolvedValue({
        goal: threadGoal({ objective: "Ship goal mode from menu" }),
        mode: "reviewed",
      });
    render(<App />);

    await screen.findByText("Session restored");
    const actionButton = screen.getByRole("button", {
      name: "Open composer actions",
    });
    expect(actionButton).not.toHaveClass("rounded-full");
    expect(
      screen.queryByRole("button", { name: "Attach context" }),
    ).not.toBeInTheDocument();

    await user.click(actionButton);

    expect(
      screen.getByRole("menuitem", { name: "Add photos" }),
    ).not.toHaveAttribute("aria-disabled", "true");
    expect(
      screen.getByRole("menuitem", { name: "Attach Google Chrome" }),
    ).toHaveAttribute("aria-disabled", "true");
    expect(
      screen.queryByRole("menuitem", { name: "添加照片" }),
    ).not.toBeInTheDocument();
    const planModeItem = screen.getByRole("menuitemcheckbox", {
      name: /Plan mode/,
    });
    expect(planModeItem).toHaveAttribute("aria-checked", "false");
    await user.click(screen.getByRole("menuitem", { name: /Goal/ }));
    expect(
      screen.getByRole("radio", { name: "Goal mode Standard" }),
    ).toHaveAttribute("aria-checked", "true");
    await user.click(screen.getByRole("radio", { name: "Goal mode Reviewed" }));
    expect(
      screen.getByRole("radio", { name: "Goal mode Reviewed" }),
    ).toHaveAttribute("aria-checked", "true");
    await user.type(
      screen.getByLabelText("Goal objective"),
      "Ship goal mode from menu",
    );
    await user.click(screen.getByRole("button", { name: "Save goal" }));

    expect(setThreadGoal).toHaveBeenCalledWith(
      "project-exagent",
      "session-desktop",
      {
        objective: "Ship goal mode from menu",
        status: "active",
        tokenBudget: null,
        clearTokenBudget: true,
        mode: "reviewed",
      },
    );
    expect(screen.getByText("Ship goal mode from menu")).toBeInTheDocument();
    expect(screen.getByText("reviewed")).toBeInTheDocument();

    await user.click(actionButton);
    expect(screen.getByRole("menuitem", { name: /Plugins/ })).toHaveAttribute(
      "aria-disabled",
      "true",
    );

    const reopenedPlanModeItem = screen.getByRole("menuitemcheckbox", {
      name: /Plan mode/,
    });
    await user.click(reopenedPlanModeItem);

    expect(
      screen.getByRole("button", { name: "Plan mode enabled" }),
    ).toBeInTheDocument();
  });

  it("opens a slash command menu and compacts the active thread", async () => {
    const user = userEvent.setup();
    const compactThread = vi.spyOn(exagentClient, "compactThread").mockResolvedValue({
      thread_id: "session-desktop",
      latest_compaction: { summary: "Slash compact summary" },
    });
    render(<App />);

    await screen.findByText("Session restored");
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-desktop",
      events: [
        {
          event_id: "evt-slash-compact",
          thread_id: "session-desktop",
          kind: {
            type: "compaction_written",
            summary: { summary: "Slash compact summary" },
          },
        },
      ],
    });
    const composer = screen.getByLabelText("Message ExAgent");
    await user.type(composer, "/");

    await user.click(screen.getByRole("menuitem", { name: /\/compact/ }));

    expect(compactThread).toHaveBeenCalledWith(
      "project-exagent",
      "session-desktop",
    );
    expect(composer).toHaveValue("");
    await user.click(await screen.findByRole("button", { name: /Events/ }));
    expect(await screen.findByText("Slash compact summary")).toBeInTheDocument();
  });

  it("runs the visible slash command with Enter", async () => {
    const user = userEvent.setup();
    const compactThread = vi.spyOn(exagentClient, "compactThread").mockResolvedValue({
      thread_id: "session-desktop",
      latest_compaction: { summary: "Slash compact summary" },
    });
    render(<App />);

    await screen.findByText("Session restored");
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-desktop",
      events: [],
    });

    await user.type(screen.getByLabelText("Message ExAgent"), "/c{Enter}");

    expect(compactThread).toHaveBeenCalledWith(
      "project-exagent",
      "session-desktop",
    );
  });

  it("uses Chinese labels for composer actions when Chinese is selected", async () => {
    const user = userEvent.setup();
    window.localStorage.setItem("exagent.locale", "zh");
    render(
      <I18nProvider>
        <App />
      </I18nProvider>,
    );

    await screen.findByText("Session restored");
    await user.click(
      screen.getByRole("button", { name: "Open composer actions" }),
    );

    expect(
      screen.getByRole("menuitem", { name: "添加照片" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("menuitem", { name: "附加 Google Chrome" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("menuitemcheckbox", { name: /计划模式/ }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("menuitem", { name: /追求目标/ }),
    ).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /插件/ })).toBeInTheDocument();
  });

  it("sends the composer with Enter and keeps Shift+Enter as a newline", async () => {
    const user = userEvent.setup();
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "session-desktop",
      turn: {
        id: "turn-enter-submit",
        status: "in_progress",
        items: [],
      },
    });
    render(<App />);

    await screen.findByText("Session restored");
    const composer = screen.getByLabelText("Message ExAgent");
    await user.type(composer, "Line one");
    await user.keyboard("{Shift>}{Enter}{/Shift}");
    await user.type(composer, "Line two");

    expect(composer).toHaveValue("Line one\nLine two");
    expect(startTurn).not.toHaveBeenCalled();

    await user.keyboard("{Enter}");

    await waitFor(() => {
      expect(startTurn).toHaveBeenCalledWith(
        "project-exagent",
        "session-desktop",
        "Line one\nLine two",
        {
          model: {
            provider_id: "openai",
            model_id: "gpt-5.5",
          },
          thinkingMode: null,
          clearThinkingMode: false,
          turnMode: "default",
        },
      );
    });
  });

  it("sends selected photos as structured local image input", async () => {
    const user = userEvent.setup();
    vi.spyOn(exagentClient, "pickImageFiles").mockResolvedValue([
      "/tmp/exagent-screenshot.png",
    ]);
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "session-desktop",
      turn: {
        id: "turn-image-submit",
        status: "in_progress",
        items: [],
      },
    });
    render(<App />);

    await screen.findByText("Session restored");
    await user.click(
      screen.getByRole("button", { name: "Open composer actions" }),
    );
    await user.click(
      screen.getByRole("menuitem", { name: /Add photos|添加照片/ }),
    );
    expect(await screen.findByText("exagent-screenshot.png")).toBeInTheDocument();

    await user.type(screen.getByLabelText("Message ExAgent"), "Use this screenshot");
    await user.click(screen.getByRole("button", { name: "Send" }));

    expect(startTurn).toHaveBeenCalledWith(
      "project-exagent",
      "session-desktop",
      "Use this screenshot",
      {
        model: {
          provider_id: "openai",
          model_id: "gpt-5.5",
        },
        thinkingMode: null,
        clearThinkingMode: false,
        turnMode: "default",
        input: [
          { type: "text", text: "Use this screenshot" },
          {
            type: "local_image",
            path: "/tmp/exagent-screenshot.png",
            detail: "high",
          },
        ],
      },
    );
    expect(screen.getByAltText("exagent-screenshot.png")).toBeInTheDocument();
  });

  it("attaches pasted image files through the desktop cache", async () => {
    const file = new File([new Uint8Array([137, 80, 78, 71])], "clipboard.png", {
      type: "image/png",
    });
    const importImageFiles = vi
      .spyOn(exagentClient, "importImageFiles")
      .mockResolvedValue(["/tmp/exagent-cache/clipboard.png"]);
    render(<App />);

    await screen.findByText("Session restored");
    fireEvent.paste(screen.getByLabelText("Message ExAgent"), {
      clipboardData: {
        files: [file],
      },
    });

    await waitFor(() => {
      expect(importImageFiles).toHaveBeenCalledWith([file]);
    });
    expect(await screen.findByText("clipboard.png")).toBeInTheDocument();
  });

  it("attaches dropped image paths through the desktop cache", async () => {
    let dragDropHandlers:
      | Parameters<typeof exagentClient.subscribeImageDragDrop>[0]
      | undefined;
    vi.spyOn(exagentClient, "subscribeImageDragDrop").mockImplementation(
      async (handlers) => {
        dragDropHandlers = handlers;
        return () => undefined;
      },
    );
    const importImagePaths = vi
      .spyOn(exagentClient, "importImagePaths")
      .mockResolvedValue(["/tmp/exagent-cache/drop.jpg"]);
    render(<App />);

    await screen.findByText("Session restored");
    expect(dragDropHandlers).toBeDefined();
    act(() => {
      dragDropHandlers!.onDrop([
        "/Users/me/Desktop/drop.jpg",
        "/Users/me/Desktop/notes.txt",
      ]);
    });

    await waitFor(() => {
      expect(importImagePaths).toHaveBeenCalledWith([
        "/Users/me/Desktop/drop.jpg",
      ]);
    });
    expect(await screen.findByText("drop.jpg")).toBeInTheDocument();
  });

  it("handles native drag-drop only from the latest mounted composer", async () => {
    let dragDropHandlers:
      | Parameters<typeof exagentClient.subscribeImageDragDrop>[0]
      | undefined;
    const subscribeImageDragDrop = vi.spyOn(exagentClient, "subscribeImageDragDrop").mockImplementation(
      async (handlers) => {
        dragDropHandlers = handlers;
        return () => undefined;
      },
    );
    const importImagePaths = vi
      .spyOn(exagentClient, "importImagePaths")
      .mockResolvedValue(["/tmp/exagent-cache/drop.jpg"]);
    const firstState = {
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project-exagent",
      activeSessionId: "session-desktop",
      activeProviderId: "deepseek",
      providerSettings: deepSeekProviderSettings({
        model_options: [
          {
            provider_id: "deepseek",
            id: "deepseek-v4-flash",
            display_name: "deepseek-v4-flash",
            context_window: null,
            supports_tools: true,
            capabilities: {
              supports_tools: true,
              input_modalities: ["text"],
              thinking: {
                supported: false,
                modes: [],
              },
            },
          },
        ],
      }),
      selectedModel: {
        provider_id: "deepseek",
        model_id: "deepseek-v4-flash",
      },
    };
    const latestState = {
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      activeProjectId: "project-exagent",
      activeSessionId: "session-desktop",
      selectedModel: {
        provider_id: "openai",
        model_id: "gpt-5.5",
      },
    };

    render(
      <I18nProvider>
        <Composer state={firstState} />
        <Composer state={latestState} />
      </I18nProvider>,
    );

    await waitFor(() => {
      expect(dragDropHandlers).toBeDefined();
    });
    expect(subscribeImageDragDrop).toHaveBeenCalledTimes(1);

    act(() => {
      dragDropHandlers!.onDrop(["/Users/me/Desktop/drop.jpg"]);
    });

    await waitFor(() => {
      expect(importImagePaths).toHaveBeenCalledTimes(1);
    });
    expect(importImagePaths).toHaveBeenCalledWith([
      "/Users/me/Desktop/drop.jpg",
    ]);
    expect(
      screen.queryByText(/Selected model accepts text only/),
    ).not.toBeInTheDocument();
  });

  it("retries native drag-drop subscription after a rejection while mounted", async () => {
    vi.useFakeTimers();
    try {
      let dragDropHandlers:
        | Parameters<typeof exagentClient.subscribeImageDragDrop>[0]
        | undefined;
      const subscribeImageDragDrop = vi
        .spyOn(exagentClient, "subscribeImageDragDrop")
        .mockRejectedValueOnce(new Error("listen failed"))
        .mockImplementationOnce(async (handlers) => {
          dragDropHandlers = handlers;
          return () => undefined;
        });
      const importImagePaths = vi
        .spyOn(exagentClient, "importImagePaths")
        .mockResolvedValue(["/tmp/exagent-cache/drop.jpg"]);
      const state = {
        ...useWorkbenchStore.getInitialState(),
        loading: false,
        activeProjectId: "project-exagent",
        activeSessionId: "session-desktop",
        selectedModel: {
          provider_id: "openai",
          model_id: "gpt-5.5",
        },
      };

      render(
        <I18nProvider>
          <Composer state={state} />
        </I18nProvider>,
      );

      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(subscribeImageDragDrop).toHaveBeenCalledTimes(1);
      expect(dragDropHandlers).toBeUndefined();

      await act(async () => {
        vi.advanceTimersByTime(1000);
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(subscribeImageDragDrop).toHaveBeenCalledTimes(2);
      expect(dragDropHandlers).toBeDefined();

      await act(async () => {
        dragDropHandlers!.onDrop(["/Users/me/Desktop/drop.jpg"]);
        await Promise.resolve();
      });
      expect(importImagePaths).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("rejects pasted images before import when the selected model is text only", async () => {
    const file = new File([new Uint8Array([137, 80, 78, 71])], "blocked.png", {
      type: "image/png",
    });
    const importImageFiles = vi.spyOn(exagentClient, "importImageFiles");
    render(<App />);

    await screen.findByText("Session restored");
    act(() => {
      useWorkbenchStore.setState({
        activeProviderId: "deepseek",
        selectedModel: {
          provider_id: "deepseek",
          model_id: "deepseek-v4-flash",
        },
        providerSettings: deepSeekProviderSettings({
          model_options: [
            {
              provider_id: "deepseek",
              id: "deepseek-v4-flash",
              display_name: "deepseek-v4-flash",
              context_window: null,
              supports_tools: true,
              capabilities: {
                supports_tools: true,
                input_modalities: ["text"],
                thinking: {
                  supported: false,
                  modes: [],
                },
              },
            },
          ],
        }),
      });
    });
    fireEvent.paste(screen.getByLabelText("Message ExAgent"), {
      clipboardData: {
        files: [file],
      },
    });

    expect(importImageFiles).not.toHaveBeenCalled();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Selected model accepts text only",
    );
  });

  it("does not restore a stale text-only image warning after switching through a vision model", async () => {
    const file = new File([new Uint8Array([137, 80, 78, 71])], "blocked.png", {
      type: "image/png",
    });
    const importImageFiles = vi.spyOn(exagentClient, "importImageFiles");
    render(<App />);

    await screen.findByText("Session restored");
    const visionProviderSettings = useWorkbenchStore.getState().providerSettings;
    act(() => {
      useWorkbenchStore.setState({
        activeProviderId: "deepseek",
        selectedModel: {
          provider_id: "deepseek",
          model_id: "deepseek-v4-flash",
        },
        providerSettings: deepSeekProviderSettings({
          model_options: [
            {
              provider_id: "deepseek",
              id: "deepseek-v4-flash",
              display_name: "deepseek-v4-flash",
              context_window: null,
              supports_tools: true,
              capabilities: {
                supports_tools: true,
                input_modalities: ["text"],
                thinking: {
                  supported: false,
                  modes: [],
                },
              },
            },
          ],
        }),
      });
    });
    fireEvent.paste(screen.getByLabelText("Message ExAgent"), {
      clipboardData: {
        files: [file],
      },
    });

    expect(importImageFiles).not.toHaveBeenCalled();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Selected model accepts text only",
    );

    act(() => {
      useWorkbenchStore.setState({
        activeProviderId: "openai",
        selectedModel: {
          provider_id: "openai",
          model_id: "gpt-5.5",
        },
        providerSettings: visionProviderSettings,
        composerAttachments: [],
      });
    });
    await waitFor(() => {
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    });

    act(() => {
      useWorkbenchStore.setState({
        activeProviderId: "deepseek",
        selectedModel: {
          provider_id: "deepseek",
          model_id: "deepseek-v4-flash",
        },
        providerSettings: deepSeekProviderSettings({
          model_options: [
            {
              provider_id: "deepseek",
              id: "deepseek-v4-flash",
              display_name: "deepseek-v4-flash",
              context_window: null,
              supports_tools: true,
              capabilities: {
                supports_tools: true,
                input_modalities: ["text"],
                thinking: {
                  supported: false,
                  modes: [],
                },
              },
            },
          ],
        }),
        composerAttachments: [],
      });
    });
    await waitFor(() => {
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    });
  });

  it("shows a warning when a pasted image fails to import", async () => {
    const file = new File([new Uint8Array([137, 80, 78, 71])], "broken.png", {
      type: "image/png",
    });
    vi.spyOn(exagentClient, "importImageFiles").mockRejectedValue(
      new Error("file is too large"),
    );
    render(<App />);

    await screen.findByText("Session restored");
    fireEvent.paste(screen.getByLabelText("Message ExAgent"), {
      clipboardData: {
        files: [file],
      },
    });

    expect(await screen.findByText(/file is too large/)).toBeInTheDocument();
  });

  it("shows the text-only warning instead of a stale pasted-image import error", async () => {
    const file = new File([new Uint8Array([137, 80, 78, 71])], "broken.png", {
      type: "image/png",
    });
    vi.spyOn(exagentClient, "importImageFiles").mockRejectedValueOnce(
      new Error("file is too large"),
    );
    render(<App />);

    await screen.findByText("Session restored");
    fireEvent.paste(screen.getByLabelText("Message ExAgent"), {
      clipboardData: {
        files: [file],
      },
    });
    expect(await screen.findByText(/file is too large/)).toBeInTheDocument();

    act(() => {
      useWorkbenchStore.setState({
        activeProviderId: "deepseek",
        selectedModel: {
          provider_id: "deepseek",
          model_id: "deepseek-v4-flash",
        },
        providerSettings: deepSeekProviderSettings({
          model_options: [
            {
              provider_id: "deepseek",
              id: "deepseek-v4-flash",
              display_name: "deepseek-v4-flash",
              context_window: null,
              supports_tools: true,
              capabilities: {
                supports_tools: true,
                input_modalities: ["text"],
                thinking: {
                  supported: false,
                  modes: [],
                },
              },
            },
          ],
        }),
        composerAttachments: [
          {
            id: "attachment-one",
            type: "local_image",
            path: "/tmp/blocked.png",
            name: "blocked.png",
            detail: "high",
          },
        ],
      });
    });

    expect(screen.getByRole("alert")).toHaveTextContent(
      "Selected model accepts text only",
    );
    expect(screen.getByRole("alert")).not.toHaveTextContent(
      "file is too large",
    );
  });

  it("restores the draft and removes the optimistic message when starting an image turn fails", async () => {
    const startTurn = vi
      .spyOn(exagentClient, "startTurn")
      .mockRejectedValue(new Error("could not read image"));
    const existingMessage = {
      id: "existing-message",
      role: "assistant" as const,
      body: "Earlier reply",
      timestamp: "history",
      threadId: "session-desktop",
    };
    const attachment = {
      id: "attachment-one",
      type: "local_image" as const,
      path: "/tmp/missing.png",
      name: "missing.png",
      detail: "high" as const,
    };
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: "session-desktop",
      composerValue: "Describe this",
      composerAttachments: [attachment],
      composerPlanMode: true,
      transcript: [existingMessage],
      sessions: [
        {
          id: "session-desktop",
          projectId: "project-exagent",
          title: "Desktop session",
          updatedAt: "now",
          status: "idle",
        },
      ],
    });

    await useWorkbenchStore.getState().sendPrompt();

    expect(startTurn).toHaveBeenCalled();
    expect(useWorkbenchStore.getState().error).toContain("could not read image");
    expect(useWorkbenchStore.getState().composerValue).toBe("Describe this");
    expect(useWorkbenchStore.getState().composerAttachments).toEqual([attachment]);
    expect(useWorkbenchStore.getState().composerPlanMode).toBe(true);
    expect(useWorkbenchStore.getState().transcript).toEqual([existingMessage]);
    expect(useWorkbenchStore.getState().sessions[0]?.status).toBe("idle");
  });

  it("deduplicates repeated image paths from the same picker result", () => {
    useWorkbenchStore.getState().addComposerAttachments([
      "/tmp/repeated.png",
      "/tmp/repeated.png",
      " /tmp/repeated.png ",
    ]);

    expect(useWorkbenchStore.getState().composerAttachments).toHaveLength(1);
    expect(useWorkbenchStore.getState().composerAttachments[0]?.path).toBe(
      "/tmp/repeated.png",
    );
  });

  it("sends plan mode with the next prompt and clears the composer toggle", async () => {
    const user = userEvent.setup();
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "session-desktop",
      turn: {
        id: "turn-plan-mode",
        status: "in_progress",
        items: [],
      },
    });
    render(<App />);

    await screen.findByText("Session restored");
    await user.click(
      screen.getByRole("button", { name: "Open composer actions" }),
    );
    await user.click(
      screen.getByRole("menuitemcheckbox", { name: /Plan mode/ }),
    );
    expect(
      screen.getByRole("button", { name: "Plan mode enabled" }),
    ).toBeInTheDocument();

    await user.type(
      screen.getByLabelText("Message ExAgent"),
      "Plan the migration",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));

    expect(startTurn).toHaveBeenCalledWith(
      "project-exagent",
      "session-desktop",
      "Plan the migration",
      {
        model: {
          provider_id: "openai",
          model_id: "gpt-5.5",
        },
        thinkingMode: null,
        clearThinkingMode: false,
        turnMode: "plan",
      },
    );
    expect(
      screen.queryByRole("button", { name: "Plan mode enabled" }),
    ).not.toBeInTheDocument();
  });

  it("updates the goal control from runtime goal events", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    act(() => {
      useWorkbenchStore.getState().applyRuntimeEvent({
        event_id: "goal-updated-event",
        thread_id: "session-desktop",
        turn_id: null,
        kind: {
          type: "thread_goal_updated",
          goal: threadGoal({ objective: "Runtime event goal" }),
        },
      });
    });

    expect(screen.getByText("Runtime event goal")).toBeInTheDocument();
    expect(screen.queryByText("reviewed")).not.toBeInTheDocument();

    act(() => {
      useWorkbenchStore.getState().applyRuntimeEvent({
        event_id: "goal-mode-updated-event",
        thread_id: "session-desktop",
        turn_id: null,
        kind: {
          type: "thread_goal_mode_updated",
          thread_id: "session-desktop",
          goal_id: "goal-desktop",
          mode: "reviewed",
        },
      });
    });

    expect(screen.getByText("reviewed")).toBeInTheDocument();

    act(() => {
      useWorkbenchStore.getState().applyRuntimeEvent({
        event_id: "goal-cleared-event",
        thread_id: "session-desktop",
        turn_id: null,
        kind: {
          type: "thread_goal_cleared",
          thread_id: "session-desktop",
        },
      });
    });

    expect(screen.queryByText("Runtime event goal")).not.toBeInTheDocument();
    expect(screen.queryByText("reviewed")).not.toBeInTheDocument();
  });

  it("clears the current goal when a completion report arrives", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    act(() => {
      useWorkbenchStore.getState().applyRuntimeEvent({
        event_id: "goal-updated-before-report",
        thread_id: "session-desktop",
        turn_id: "turn-goal-complete",
        kind: {
          type: "thread_goal_updated",
          goal: threadGoal({
            goal_id: "goal-complete",
            objective: "List all available tools",
            status: "active",
          }),
        },
      });
      useWorkbenchStore.getState().applyRuntimeEvent({
        event_id: "goal-mode-before-report",
        thread_id: "session-desktop",
        turn_id: "turn-goal-complete",
        kind: {
          type: "thread_goal_mode_updated",
          thread_id: "session-desktop",
          goal_id: "goal-complete",
          mode: "intensive",
        },
      });
    });

    expect(screen.getByText("List all available tools")).toBeInTheDocument();
    expect(screen.getByText("intensive")).toBeInTheDocument();

    act(() => {
      useWorkbenchStore.getState().applyRuntimeEvent({
        event_id: "goal-complete-report",
        thread_id: "session-desktop",
        turn_id: "turn-goal-complete",
        kind: {
          type: "thread_goal_report",
          report: {
            goal_id: "goal-complete",
            objective: "List all available tools",
            final_status: "complete",
            turns_run: 1,
            tokens_used: 8755,
            time_used_seconds: 9,
            pending_approvals_count: 0,
            summary: "Listed all available tools.",
          },
        },
      });
    });

    expect(useWorkbenchStore.getState().currentGoal).toBeNull();
    expect(useWorkbenchStore.getState().currentGoalMode).toBe("standard");
    expect(screen.queryByText("intensive")).not.toBeInTheDocument();
    expect(screen.getByRole("article", { name: "Goal report" })).toBeInTheDocument();
    expect(screen.getByText("List all available tools")).toBeInTheDocument();
    expect(screen.getByText("8,755 tokens")).toBeInTheDocument();
    expect(screen.getByText("9s")).toBeInTheDocument();
  });

  it("lets a draft session save a goal and applies it when the first prompt creates a thread", async () => {
    const user = userEvent.setup();
    vi.spyOn(exagentClient, "getWorkbenchSnapshot").mockResolvedValue({
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Volumes/EXEXEX/ExAgent",
          active: true,
        },
      ],
      sessions: [],
      activeProjectId: "project-exagent",
      activeSessionId: null,
      transcript: [],
      events: [],
      changedFiles: [],
      cwd: "/Volumes/EXEXEX/ExAgent",
      policy: "local",
      tokenUsage: {
        input: 0,
        output: 0,
        limit: 1,
      },
      tokenUsageByThreadId: {},
      runtimeSettings: null,
      selectedModel: null,
      selectedThinkingMode: null,
    });
    vi.spyOn(exagentClient, "getRuntimeSettings").mockResolvedValue({
      default_model: "gpt-5.5",
      default_thinking_mode: "medium",
      presets: [],
      mcp_servers: [],
      skill_roots: [],
    });
    vi.spyOn(exagentClient, "getProviderSettings").mockResolvedValue(
      deepSeekProviderSettings(),
    );
    const setThreadGoal = vi
      .spyOn(exagentClient, "setThreadGoal")
      .mockResolvedValue({
        goal: threadGoal({
          thread_id: "session-created",
          objective: "Ship the draft goal",
          token_budget: 1200,
        }),
        mode: "intensive",
      });
    vi.spyOn(exagentClient, "startThread").mockResolvedValue({
      thread: {
        id: "session-created",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [],
      },
    });
    vi.spyOn(exagentClient, "listThreads").mockResolvedValue([
      threadRecord({
        id: "session-created",
        project_id: "project-exagent",
        fallback_title: "New session",
      }),
    ]);
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "session-created",
      turn: {
        id: "turn-created",
        status: "in_progress",
        items: [],
      },
    });
    render(<App />);

    await screen.findByText("What should we build in ExAgent?");
    await user.click(screen.getByRole("button", { name: "Open composer actions" }));
    const goalItem = screen.getByRole("menuitem", { name: /Goal/ });
    expect(goalItem).not.toHaveAttribute("aria-disabled", "true");

    await user.click(goalItem);
    await user.click(screen.getByRole("radio", { name: "Goal mode Intensive" }));
    await user.type(screen.getByLabelText("Goal objective"), "Ship the draft goal");
    await user.type(screen.getByLabelText("Goal token budget"), "1200");
    await user.click(screen.getByRole("button", { name: "Save goal" }));

    expect(setThreadGoal).not.toHaveBeenCalled();
    await waitFor(() => {
      expect(useWorkbenchStore.getState().draftGoal?.objective).toBe(
        "Ship the draft goal",
      );
    });
    expect(useWorkbenchStore.getState().draftGoal?.mode).toBe("intensive");
    expect(await screen.findByText("Ship the draft goal")).toBeInTheDocument();
    expect(screen.getByText("intensive")).toBeInTheDocument();

    await user.type(screen.getByLabelText("Message ExAgent"), "Build it");
    await user.click(screen.getByRole("button", { name: "Send" }));

    expect(setThreadGoal).toHaveBeenCalledWith("project-exagent", "session-created", {
      objective: "Ship the draft goal",
      status: "active",
      tokenBudget: 1200,
      clearTokenBudget: false,
      mode: "intensive",
    });
    expect(setThreadGoal.mock.invocationCallOrder[0]).toBeLessThan(
      startTurn.mock.invocationCallOrder[0],
    );
    expect(useWorkbenchStore.getState().draftGoal).toBeNull();
    expect(useWorkbenchStore.getState().currentGoal?.objective).toBe(
      "Ship the draft goal",
    );
  });

  it("uses the composer action as interrupt while the active session is running", async () => {
    const user = userEvent.setup();
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "session-desktop",
      turn: {
        id: "turn-should-not-start",
        status: "in_progress",
        items: [],
      },
    });
    const interruptTurn = vi
      .spyOn(exagentClient, "interruptTurn")
      .mockResolvedValue(undefined);
    vi.spyOn(exagentClient, "agentTree").mockResolvedValue({
      root: {
        thread_id: "session-desktop",
        root_thread_id: "session-desktop",
        depth: 0,
        agent_path: "root",
        status: "idle",
        children: [],
      },
    });
    render(<App />);

    await screen.findByText("Session restored");
    act(() => {
      const current = useWorkbenchStore.getState();
      useWorkbenchStore.setState({
        composerValue: "/",
        sessions: current.sessions.map((session) =>
          session.id === "session-desktop"
            ? { ...session, status: "running" }
            : session,
        ),
      });
    });

    await user.type(screen.getByLabelText("Message ExAgent"), "{Enter}");

    expect(interruptTurn).toHaveBeenCalledWith(
      "project-exagent",
      "session-desktop",
    );
    expect(startTurn).not.toHaveBeenCalled();
    await waitFor(() => {
      expect(
        useWorkbenchStore
          .getState()
          .sessions.find((session) => session.id === "session-desktop")?.status,
      ).toBe("idle");
    });
    expect(screen.getByRole("button", { name: "Send" })).toBeInTheDocument();
  });

  it("self-heals a stale running composer when interrupt reports no active turn", async () => {
    const user = userEvent.setup();
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "session-desktop",
      turn: {
        id: "turn-should-not-start",
        status: "in_progress",
        items: [],
      },
    });
    const interruptTurn = vi
      .spyOn(exagentClient, "interruptTurn")
      .mockRejectedValue(
        new Error(
          "turn rejected for thread session-desktop: thread has no active turn",
        ),
      );
    vi.spyOn(exagentClient, "agentTree").mockResolvedValue({
      root: {
        thread_id: "session-desktop",
        root_thread_id: "session-desktop",
        depth: 0,
        agent_path: "root",
        status: "idle",
        children: [],
      },
    });
    render(<App />);

    await screen.findByText("Session restored");
    act(() => {
      const current = useWorkbenchStore.getState();
      useWorkbenchStore.setState({
        composerValue: "Do not submit while stale",
        sessions: current.sessions.map((session) =>
          session.id === "session-desktop"
            ? { ...session, status: "running" }
            : session,
        ),
      });
    });

    await user.click(screen.getByRole("button", { name: "Interrupt" }));

    expect(interruptTurn).toHaveBeenCalledWith(
      "project-exagent",
      "session-desktop",
    );
    expect(startTurn).not.toHaveBeenCalled();
    await waitFor(() => {
      expect(
        useWorkbenchStore
          .getState()
          .sessions.find((session) => session.id === "session-desktop")?.status,
      ).toBe("idle");
    });
    expect(useWorkbenchStore.getState().error).toBeNull();
  });

  it("clears unsupported selected thinking mode when switching to a model with narrower backend modes", async () => {
    const user = userEvent.setup();
    render(<App />);

    await screen.findByText("Session restored");

    act(() => {
      const currentProviderSettings =
        useWorkbenchStore.getState().providerSettings;
      useWorkbenchStore.setState({
        providerSettings: currentProviderSettings
          ? {
              ...currentProviderSettings,
              model_options: [
                ...currentProviderSettings.model_options,
                {
                  provider_id: "openai",
                  id: "gpt-5",
                  display_name: "gpt-5",
                  context_window: 400000,
                  supports_tools: true,
                  capabilities: {
                    supports_tools: true,
                    thinking: {
                      supported: true,
                      modes: ["low"],
                    },
                  },
                },
              ],
            }
          : currentProviderSettings,
        selectedModel: {
          provider_id: "openai",
          model_id: "gpt-4.1",
        },
        selectedThinkingMode: "high",
      });
    });

    const modelButton = screen.getByRole("button", { name: "Composer model" });
    await user.click(modelButton);
    await user.click(
      screen.getByRole("menuitemradio", { name: "gpt-5 Available" }),
    );

    expect(useWorkbenchStore.getState().selectedThinkingMode).toBeNull();
    expect(modelButton).toHaveTextContent("gpt-5");
    expect(
      screen.getByRole("button", { name: "Thinking mode" }),
    ).toHaveTextContent("Default");

    await user.click(screen.getByRole("button", { name: "Thinking mode" }));
    expect(
      screen.getByRole("menuitemradio", { name: "Thinking default" }),
    ).toHaveAttribute("aria-checked", "true");
    expect(
      screen.getByRole("menuitemradio", { name: "Thinking low" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("menuitemradio", { name: "Thinking high" }),
    ).not.toBeInTheDocument();
  });

  it("shows general mcp skills and archive settings tabs without runtime", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    await userEvent.click(
      screen.getByRole("button", { name: "Open settings" }),
    );

    const dialog = screen.getByRole("dialog", { name: "Settings" });
    expect(dialog).toHaveClass("h-[min(720px,calc(100dvh-64px))]");
    expect(screen.getByRole("tab", { name: "Providers" })).toBeInTheDocument();
    expect(
      screen.queryByRole("tab", { name: "Runtime" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "General" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "MCP" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Skills" })).toBeInTheDocument();
    expect(
      screen.queryByRole("tab", { name: "Language" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Archive" })).toBeInTheDocument();

    await userEvent.click(screen.getByRole("tab", { name: "General" }));
    expect(dialog).toHaveClass("h-[min(720px,calc(100dvh-64px))]");
    expect(screen.getByRole("heading", { name: "General" })).toBeInTheDocument();
    expect(screen.getByText("Theme")).toBeInTheDocument();
    expect(screen.getByText("Language")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("tab", { name: "MCP" }));
    expect(dialog).toHaveClass("h-[min(720px,calc(100dvh-64px))]");
    expect(
      screen.getByRole("button", { name: "Add MCP server" }),
    ).toBeInTheDocument();

    await userEvent.click(screen.getByRole("tab", { name: "Skills" }));
    expect(dialog).toHaveClass("h-[min(720px,calc(100dvh-64px))]");
    expect(
      screen.getByRole("button", { name: "Add global root" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Catalog" })).toBeInTheDocument();

    await userEvent.click(screen.getByRole("tab", { name: "Archive" }));
    expect(dialog).toHaveClass("h-[min(720px,calc(100dvh-64px))]");
    expect(screen.getByRole("button", { name: "Refresh" })).toBeInTheDocument();
  });

  it("changes the desktop theme from general settings", async () => {
    const user = userEvent.setup();
    render(<App />);

    await screen.findByText("Session restored");
    await user.click(screen.getByRole("button", { name: "Open settings" }));
    await user.click(screen.getByRole("tab", { name: "General" }));
    await user.click(screen.getByRole("radio", { name: /Light/ }));

    expect(document.documentElement.dataset.theme).toBe("light");
    expect(window.localStorage.getItem("exagent.theme")).toBe("light");

    await user.click(screen.getByRole("radio", { name: /System/ }));

    expect(document.documentElement.dataset.theme).toBeUndefined();
    expect(window.localStorage.getItem("exagent.theme")).toBe("system");
  });

  it("restores an archived conversation from settings", async () => {
    const user = userEvent.setup();
    const unarchiveThread = vi
      .spyOn(exagentClient, "unarchiveThread")
      .mockResolvedValue(undefined);
    vi.spyOn(exagentClient, "listProjects").mockResolvedValue([
      {
        id: "project-alpha",
        name: "Alpha",
        path: "/tmp/alpha",
        archived_at: null,
        pinned: false,
      },
    ]);
    vi.spyOn(exagentClient, "listThreads")
      .mockResolvedValueOnce([
        threadRecord({
          id: "session-archived",
          project_id: "project-alpha",
          fallback_title: "Archived alpha",
          archived_at: 10,
        }),
      ])
      .mockResolvedValueOnce([]);

    render(<App />);
    await screen.findByText("Session restored");
    await user.click(screen.getByRole("button", { name: "Open settings" }));
    await user.click(screen.getByRole("tab", { name: "Archive" }));

    expect(await screen.findByText("Archived alpha")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Restore" }));

    expect(unarchiveThread).toHaveBeenCalledWith("session-archived");
    expect(
      await screen.findByText("No archived conversations"),
    ).toBeInTheDocument();
  });

  it("opens an archived conversation after restoring it from settings", async () => {
    const user = userEvent.setup();
    const unarchiveThread = vi
      .spyOn(exagentClient, "unarchiveThread")
      .mockResolvedValue(undefined);
    vi.spyOn(exagentClient, "listProjects").mockResolvedValue([
      {
        id: "project-alpha",
        name: "Alpha",
        path: "/tmp/alpha",
        archived_at: null,
        pinned: false,
      },
    ]);
    vi.spyOn(exagentClient, "listThreads")
      .mockResolvedValueOnce([
        threadRecord({
          id: "session-archived",
          project_id: "project-alpha",
          fallback_title: "Archived alpha",
          archived_at: 10,
        }),
      ])
      .mockResolvedValueOnce([]);
    vi.spyOn(exagentClient, "reindexProject").mockResolvedValue([
      threadRecord({
        id: "session-archived",
        project_id: "project-alpha",
        fallback_title: "Archived alpha",
        archived_at: null,
      }),
    ]);
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-archived",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [
          {
            id: "turn-archived",
            status: "completed",
            items: [{ type: "assistant_message", text: "Restored transcript" }],
          },
        ],
      },
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(null);
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-archived",
      events: [],
    });
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      loading: false,
      projects: [
        {
          id: "project-alpha",
          name: "Alpha",
          path: "/tmp/alpha",
          active: true,
        },
      ],
      sessions: [],
      activeProjectId: "project-alpha",
      activeSessionId: null,
    });

    render(<App />);
    await screen.findByText("What should we build in Alpha?");
    await user.click(screen.getByRole("button", { name: "Open settings" }));
    await user.click(screen.getByRole("tab", { name: "Archive" }));

    expect(await screen.findByText("Archived alpha")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Open" }));

    expect(unarchiveThread).toHaveBeenCalledWith("session-archived");
    await waitFor(() => {
      expect(useWorkbenchStore.getState().activeProjectId).toBe(
        "project-alpha",
      );
      expect(useWorkbenchStore.getState().activeSessionId).toBe(
        "session-archived",
      );
    });
    expect(useWorkbenchStore.getState().transcript).toEqual([
      expect.objectContaining({ body: "Restored transcript" }),
    ]);
  });

  it("keeps provider connection pages fixed height and exposes OpenAI model setup", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    await userEvent.click(
      screen.getByRole("button", { name: "Open settings" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Configure OpenAI" }),
    );

    const dialog = screen.getByRole("dialog", { name: "Connect OpenAI" });
    expect(dialog).toHaveClass("h-[min(720px,calc(100dvh-64px))]");
    expect(screen.getByLabelText("Model")).toHaveValue("gpt-5.5");
    expect(
      screen.getByRole("button", { name: "Discover models" }),
    ).toBeEnabled();
    expect(screen.getByRole("button", { name: "Save provider" })).toBeEnabled();
  });

  it("shows runtime configuration in the inspector", async () => {
    render(<App />);

    await screen.findByText("Session restored");

    expect(
      screen.getByRole("heading", { name: "Runtime" }),
    ).toBeInTheDocument();
    expect(screen.getAllByText("gpt-5.5").length).toBeGreaterThan(0);
    expect(screen.getByText("default")).toBeInTheDocument();
    expect(screen.getByText("MCP servers")).toBeInTheDocument();
    expect(screen.getByText("Skill roots")).toBeInTheDocument();
  });

  it("keeps lower-priority inspector sections collapsed until requested", async () => {
    render(<App />);

    await screen.findByText("Session restored");

    const changedFilesToggle = screen.getByRole("button", {
      name: /Changed Files/,
    });
    expect(changedFilesToggle).toHaveAttribute("aria-expanded", "false");
    expect(
      screen.queryByText("No changed files reported."),
    ).not.toBeInTheDocument();

    await userEvent.click(changedFilesToggle);

    expect(changedFilesToggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("No changed files reported.")).toBeInTheDocument();
  });

  it("renders user messages as right bubbles and assistant replies as left text", async () => {
    render(<App />);

    await screen.findByText("Session restored");

    expect(screen.getAllByLabelText("User message")[0]).toHaveClass(
      "justify-end",
    );
    expect(screen.getAllByLabelText("Assistant message")[0]).toHaveClass(
      "max-w-[780px]",
    );
  });

  it("groups turn activity before the final assistant reply and collapses it after completion", async () => {
    const user = userEvent.setup();
    vi.spyOn(exagentClient, "getWorkbenchSnapshot").mockResolvedValue({
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Volumes/EXEXEX/ExAgent",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-desktop",
          projectId: "project-exagent",
          title: "Reasoning session",
          updatedAt: "now",
          status: "idle",
        },
      ],
      activeProjectId: "project-exagent",
      activeSessionId: "session-desktop",
      transcript: [
        {
          id: "user-message",
          role: "user",
          body: "Explain the provider shape",
          timestamp: "history",
          threadId: "session-desktop",
          turnId: "turn-reasoning",
          turnStatus: "completed",
        },
        {
          id: "reasoning-message",
          role: "reasoning",
          title: "Reasoning",
          body: "Checked the provider response shape.",
          timestamp: "history",
          threadId: "session-desktop",
          turnId: "turn-reasoning",
          turnStatus: "completed",
        },
        {
          id: "tool-message",
          role: "tool",
          title: "read_file",
          body: "Tool completed.",
          timestamp: "history",
          status: "success",
          threadId: "session-desktop",
          turnId: "turn-reasoning",
          turnStatus: "completed",
          toolName: "read_file",
          toolStatus: "completed",
        },
        {
          id: "assistant-message",
          role: "assistant",
          body: "Final answer.",
          timestamp: "history",
          threadId: "session-desktop",
          turnId: "turn-reasoning",
          turnStatus: "completed",
        },
      ],
      events: [],
      changedFiles: [],
      cwd: "/Volumes/EXEXEX/ExAgent",
      policy: "local",
      tokenUsage: {
        input: 0,
        output: 0,
        limit: 1,
      },
      tokenUsageByThreadId: {},
      runtimeSettings: null,
      selectedModel: null,
      selectedThinkingMode: null,
    });
    render(<App />);

    const activityGroup = await screen.findByLabelText("Turn activity");
    const activityToggle = within(activityGroup).getByRole("button", {
      name: /Activity/,
    });
    expect(activityToggle).toHaveAttribute("aria-expanded", "false");
    expect(
      screen.queryByText("Checked the provider response shape."),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("read_file")).not.toBeInTheDocument();
    expect(screen.getByText("Final answer.")).toBeInTheDocument();

    await user.click(activityToggle);

    expect(activityToggle).toHaveAttribute("aria-expanded", "true");
    expect(
      screen.getByText("Checked the provider response shape."),
    ).toBeInTheDocument();
    expect(screen.getByText("read_file")).toBeInTheDocument();
  });

  it("uses the saved provider for prompts without reloading the workbench", async () => {
    const user = userEvent.setup();
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "session-desktop",
      turn: {
        id: "turn-provider-switch",
        status: "in_progress",
        items: [],
      },
    });
    render(<App />);

    await screen.findByText("Session restored");
    await user.click(screen.getByRole("button", { name: "Open settings" }));
    await user.click(
      screen.getByRole("button", { name: "Configure OpenAI Compatible" }),
    );
    await user.click(screen.getByRole("button", { name: "Save provider" }));
    await user.keyboard("{Escape}");
    await user.type(
      screen.getByLabelText("Message ExAgent"),
      "Use the new provider",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));

    expect(startTurn).toHaveBeenCalledWith(
      "project-exagent",
      "session-desktop",
      "Use the new provider",
      {
        model: {
          provider_id: "openai_compatible",
          model_id: "local-model",
        },
        thinkingMode: null,
        clearThinkingMode: false,
        turnMode: "default",
      },
    );
  });

  it("uses backend model capabilities to omit thinking for prompts", async () => {
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "session-desktop",
      turn: {
        id: "turn-capability",
        status: "in_progress",
        items: [],
      },
    });
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: "session-desktop",
      composerValue: "Use backend capabilities",
      selectedModel: {
        provider_id: "openai",
        model_id: "gpt-5-disabled",
      },
      selectedThinkingMode: null,
      runtimeSettings: {
        default_model: "gpt-4.1",
        default_thinking_mode: "high",
        presets: [],
        mcp_servers: [],
        skill_roots: [],
      },
      providerSettings: {
        providers: [],
        active_provider_id: "openai",
        config: {
          provider_id: "openai",
          base_url: "https://api.openai.com/v1",
          model: "gpt-5-disabled",
          has_api_key: false,
          credential_source: "none",
          auth_required: true,
        },
        connected_provider: null,
        last_connection: null,
        configured_providers: [],
        model_options: [
          {
            provider_id: "openai",
            id: "gpt-5-disabled",
            display_name: "gpt-5-disabled",
            context_window: null,
            supports_tools: true,
            capabilities: {
              supports_tools: true,
              thinking: {
                supported: false,
                modes: [],
              },
            },
          },
        ],
      },
    });

    await useWorkbenchStore.getState().sendPrompt();

    expect(startTurn).toHaveBeenCalledWith(
      "project-exagent",
      "session-desktop",
      "Use backend capabilities",
      {
        model: {
          provider_id: "openai",
          model_id: "gpt-5-disabled",
        },
        thinkingMode: null,
        clearThinkingMode: true,
        turnMode: "default",
      },
    );
  });

  it("blocks image input before starting a turn when the selected model is text-only", async () => {
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "session-desktop",
      turn: {
        id: "turn-blocked-image",
        status: "in_progress",
        items: [],
      },
    });
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: "session-desktop",
      composerValue: "Describe this",
      composerAttachments: [
        {
          id: "attachment-one",
          type: "local_image",
          path: "/tmp/blocked.png",
          name: "blocked.png",
          detail: "high",
        },
      ],
      selectedModel: {
        provider_id: "deepseek",
        model_id: "deepseek-v4-flash",
      },
      selectedThinkingMode: null,
      runtimeSettings: {
        default_model: "deepseek-v4-flash",
        default_thinking_mode: null,
        presets: [],
        mcp_servers: [],
        skill_roots: [],
      },
      providerSettings: deepSeekProviderSettings({
        model_options: [
          {
            provider_id: "deepseek",
            id: "deepseek-v4-flash",
            display_name: "deepseek-v4-flash",
            context_window: null,
            supports_tools: true,
            capabilities: {
              supports_tools: true,
              input_modalities: ["text"],
              thinking: {
                supported: false,
                modes: [],
              },
            },
          },
        ],
      }),
    });

    await useWorkbenchStore.getState().sendPrompt();

    expect(startTurn).not.toHaveBeenCalled();
    expect(useWorkbenchStore.getState().error).toContain(
      "does not support image input",
    );
    expect(useWorkbenchStore.getState().composerAttachments).toHaveLength(1);
  });

  it("shows image input as unavailable in the composer for text-only models", async () => {
    const user = userEvent.setup();
    render(<App />);

    await screen.findByText("Session restored");
    act(() => {
      useWorkbenchStore.setState({
        activeProviderId: "deepseek",
        selectedModel: {
          provider_id: "deepseek",
          model_id: "deepseek-v4-flash",
        },
        providerSettings: deepSeekProviderSettings({
          model_options: [
            {
              provider_id: "deepseek",
              id: "deepseek-v4-flash",
              display_name: "deepseek-v4-flash",
              context_window: null,
              supports_tools: true,
              capabilities: {
                supports_tools: true,
                input_modalities: ["text"],
                thinking: {
                  supported: false,
                  modes: [],
                },
              },
            },
          ],
        }),
        composerValue: "Describe this",
        composerAttachments: [
          {
            id: "attachment-one",
            type: "local_image",
            path: "/tmp/blocked.png",
            name: "blocked.png",
            detail: "high",
          },
        ],
      });
    });

    expect(screen.getByRole("alert")).toHaveTextContent(
      "Selected model accepts text only",
    );
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();

    await user.click(
      screen.getByRole("button", { name: "Open composer actions" }),
    );
    expect(
      screen.getByRole("menuitem", { name: /Add photos/ }),
    ).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText("Text only")).toBeInTheDocument();
  });

  it("does not submit blocked image input from the Enter shortcut", async () => {
    const user = userEvent.setup();
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "session-desktop",
      turn: {
        id: "turn-should-not-start",
        status: "in_progress",
        items: [],
      },
    });
    render(<App />);

    await screen.findByText("Session restored");
    act(() => {
      useWorkbenchStore.setState({
        activeProviderId: "deepseek",
        selectedModel: {
          provider_id: "deepseek",
          model_id: "deepseek-v4-flash",
        },
        providerSettings: deepSeekProviderSettings({
          model_options: [
            {
              provider_id: "deepseek",
              id: "deepseek-v4-flash",
              display_name: "deepseek-v4-flash",
              context_window: null,
              supports_tools: true,
              capabilities: {
                supports_tools: true,
                input_modalities: ["text"],
                thinking: {
                  supported: false,
                  modes: [],
                },
              },
            },
          ],
        }),
        composerValue: "Describe this",
        composerAttachments: [
          {
            id: "attachment-one",
            type: "local_image",
            path: "/tmp/blocked.png",
            name: "blocked.png",
            detail: "high",
          },
        ],
        error: null,
      });
    });

    await user.click(screen.getByLabelText("Message ExAgent"));
    await user.keyboard("{Enter}");

    expect(startTurn).not.toHaveBeenCalled();
    expect(useWorkbenchStore.getState().error).toBeNull();
  });

  it("omits unsupported runtime default thinking mode for low-only models", async () => {
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "session-desktop",
      turn: {
        id: "turn-low-only",
        status: "in_progress",
        items: [],
      },
    });
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: "session-desktop",
      composerValue: "Use low-only model",
      activeProviderId: "openai",
      selectedModel: {
        provider_id: "openai",
        model_id: "gpt-5-low-only",
      },
      selectedThinkingMode: null,
      runtimeSettings: {
        default_model: "gpt-4.1",
        default_thinking_mode: "high",
        presets: [],
        mcp_servers: [],
        skill_roots: [],
      },
      providerSettings: {
        providers: [],
        active_provider_id: "openai",
        config: {
          provider_id: "openai",
          base_url: "https://api.openai.com/v1",
          model: "gpt-5-low-only",
          has_api_key: false,
          credential_source: "none",
          auth_required: true,
        },
        connected_provider: null,
        last_connection: null,
        configured_providers: [],
        model_options: [
          {
            provider_id: "openai",
            id: "gpt-5-low-only",
            display_name: "gpt-5-low-only",
            context_window: null,
            supports_tools: true,
            capabilities: {
              supports_tools: true,
              thinking: {
                supported: true,
                modes: ["low"],
              },
            },
          },
        ],
      },
    });

    await useWorkbenchStore.getState().sendPrompt();

    expect(startTurn).toHaveBeenCalledWith(
      "project-exagent",
      "session-desktop",
      "Use low-only model",
      {
        model: {
          provider_id: "openai",
          model_id: "gpt-5-low-only",
        },
        thinkingMode: null,
        clearThinkingMode: true,
        turnMode: "default",
      },
    );
  });

  it("inherits backend thinking defaults when model capabilities are unknown", async () => {
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "session-desktop",
      turn: {
        id: "turn-unknown-capability",
        status: "in_progress",
        items: [],
      },
    });
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: "session-desktop",
      composerValue: "Use unknown model",
      activeProviderId: "openai_compatible",
      selectedModel: {
        provider_id: "openai_compatible",
        model_id: "manual-model",
      },
      selectedThinkingMode: "high",
      runtimeSettings: {
        default_model: "gpt-4.1",
        default_thinking_mode: "high",
        presets: [],
        mcp_servers: [],
        skill_roots: [],
      },
      providerSettings: {
        providers: [],
        active_provider_id: "openai_compatible",
        config: {
          provider_id: "openai_compatible",
          base_url: "http://127.0.0.1:11434/v1",
          model: "manual-model",
          has_api_key: false,
          credential_source: "none",
          auth_required: false,
        },
        connected_provider: null,
        last_connection: null,
        configured_providers: [],
        model_options: [],
      },
    });

    await useWorkbenchStore.getState().sendPrompt();

    expect(startTurn).toHaveBeenCalledWith(
      "project-exagent",
      "session-desktop",
      "Use unknown model",
      {
        model: {
          provider_id: "openai_compatible",
          model_id: "manual-model",
        },
        thinkingMode: null,
        clearThinkingMode: false,
        turnMode: "default",
      },
    );
    expect(useWorkbenchStore.getState().selectedThinkingMode).toBeNull();
  });

  it("clears stale unsupported selected thinking mode from store when sending a prompt", async () => {
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "session-desktop",
      turn: {
        id: "turn-stale-thinking",
        status: "in_progress",
        items: [],
      },
    });
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: "session-desktop",
      composerValue: "Clear stale thinking",
      activeProviderId: "openai",
      selectedModel: {
        provider_id: "openai",
        model_id: "gpt-5-low-only",
      },
      selectedThinkingMode: "high",
      runtimeSettings: {
        default_model: "gpt-4.1",
        default_thinking_mode: null,
        presets: [],
        mcp_servers: [],
        skill_roots: [],
      },
      providerSettings: {
        providers: [],
        active_provider_id: "openai",
        config: {
          provider_id: "openai",
          base_url: "https://api.openai.com/v1",
          model: "gpt-5-low-only",
          has_api_key: false,
          credential_source: "none",
          auth_required: true,
        },
        connected_provider: null,
        last_connection: null,
        configured_providers: [],
        model_options: [
          {
            provider_id: "openai",
            id: "gpt-5-low-only",
            display_name: "gpt-5-low-only",
            context_window: null,
            supports_tools: true,
            capabilities: {
              supports_tools: true,
              thinking: {
                supported: true,
                modes: ["low"],
              },
            },
          },
        ],
      },
    });

    await useWorkbenchStore.getState().sendPrompt();

    expect(startTurn).toHaveBeenCalledWith(
      "project-exagent",
      "session-desktop",
      "Clear stale thinking",
      {
        model: {
          provider_id: "openai",
          model_id: "gpt-5-low-only",
        },
        thinkingMode: null,
        clearThinkingMode: true,
        turnMode: "default",
      },
    );
    expect(useWorkbenchStore.getState().selectedThinkingMode).toBeNull();
  });

  it("sends valid thinking modes without clearing the backend override", async () => {
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "session-desktop",
      turn: {
        id: "turn-thinking",
        status: "in_progress",
        items: [],
      },
    });
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: "session-desktop",
      composerValue: "Use high thinking",
      activeProviderId: "openai",
      selectedModel: {
        provider_id: "openai",
        model_id: "gpt-5",
      },
      selectedThinkingMode: "high",
      runtimeSettings: {
        default_model: "gpt-4.1",
        default_thinking_mode: null,
        presets: [],
        mcp_servers: [],
        skill_roots: [],
      },
      providerSettings: {
        providers: [],
        active_provider_id: "openai",
        config: {
          provider_id: "openai",
          base_url: "https://api.openai.com/v1",
          model: "gpt-5",
          has_api_key: false,
          credential_source: "none",
          auth_required: true,
        },
        connected_provider: null,
        last_connection: null,
        configured_providers: [],
        model_options: [
          {
            provider_id: "openai",
            id: "gpt-5",
            display_name: "gpt-5",
            context_window: null,
            supports_tools: true,
            capabilities: {
              supports_tools: true,
              thinking: {
                supported: true,
                modes: ["low", "high"],
              },
            },
          },
        ],
      },
    });

    await useWorkbenchStore.getState().sendPrompt();

    expect(startTurn).toHaveBeenCalledWith(
      "project-exagent",
      "session-desktop",
      "Use high thinking",
      {
        model: {
          provider_id: "openai",
          model_id: "gpt-5",
        },
        thinkingMode: "high",
        clearThinkingMode: false,
        turnMode: "default",
      },
    );
  });

  it("passes turn options through the Tauri turn_start command", async () => {
    Reflect.set(window, "__TAURI_INTERNALS__", {});

    await exagentClient.startTurn(
      "project-exagent",
      "session-desktop",
      "Clear inherited thinking",
      {
        model: {
          provider_id: "openai",
          model_id: "gpt-4.1",
        },
        clearThinkingMode: true,
        turnMode: "plan",
        input: [
          { type: "text", text: "Clear inherited thinking" },
          { type: "local_image", path: "/tmp/plan.png", detail: "high" },
        ],
      },
    );

    expect(tauriMocks.invoke).toHaveBeenCalledWith("turn_start", {
      projectId: "project-exagent",
      threadId: "session-desktop",
      prompt: "Clear inherited thinking",
      model: {
        provider_id: "openai",
        model_id: "gpt-4.1",
      },
      thinkingMode: null,
      clearThinkingMode: true,
      turnMode: "plan",
      input: [
        { type: "text", text: "Clear inherited thinking" },
        { type: "local_image", path: "/tmp/plan.png", detail: "high" },
      ],
    });
  });

  it("opens a draft session without adding a left-sidebar session", async () => {
    const startThread = vi.spyOn(exagentClient, "startThread");
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: "session-desktop",
      sessions: [
        {
          id: "session-desktop",
          projectId: "project-exagent",
          title: "Desktop GUI workbench",
          updatedAt: "local preview",
          status: "idle",
        },
      ],
      transcript: [
        {
          id: "message-existing",
          role: "assistant",
          body: "Existing session",
          timestamp: "preview",
        },
      ],
      events: [
        {
          id: "event-existing",
          label: "Existing",
          detail: "Existing event",
          timestamp: "preview",
        },
      ],
    });

    await useWorkbenchStore.getState().startSession();

    expect(startThread).not.toHaveBeenCalled();
    expect(useWorkbenchStore.getState()).toMatchObject({
      activeSessionId: null,
      transcript: [],
      events: [],
      sessions: [
        {
          id: "session-desktop",
          title: "Desktop GUI workbench",
        },
      ],
    });
  });

  it("can reopen an existing session after entering the draft new-session state", async () => {
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: "session-desktop",
      sessions: [
        {
          id: "session-desktop",
          projectId: "project-exagent",
          title: "Desktop GUI workbench",
          updatedAt: "local preview",
          status: "idle",
        },
      ],
      transcript: [
        {
          id: "message-existing",
          role: "assistant",
          body: "Existing session",
          timestamp: "preview",
        },
      ],
      events: [],
    });

    await useWorkbenchStore.getState().startSession();
    expect(useWorkbenchStore.getState()).toMatchObject({
      activeSessionId: null,
      transcript: [],
    });

    await useWorkbenchStore.getState().openSession("session-desktop");

    expect(useWorkbenchStore.getState().activeSessionId).toBe(
      "session-desktop",
    );
    expect(useWorkbenchStore.getState().transcript).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          body: expect.stringContaining("draft start state"),
        }),
      ]),
    );
  });

  it("creates the real session only when a draft prompt is sent", async () => {
    const startThread = vi
      .spyOn(exagentClient, "startThread")
      .mockResolvedValue({
        thread: {
          id: "session-created",
          status: "idle",
          goal_mode: "standard",
          active_turn: null,
          turns: [],
        },
      });
    vi.spyOn(exagentClient, "listThreads").mockResolvedValue([
      {
        id: "session-created",
        project_id: "project-exagent",
        rollout_path: "",
        user_title: "New session",
        fallback_title: "New session",
        preview: "New session",
        title_source: "mock",
        archived_at: null,
        pinned: false,
        status: "idle",
        created_at: Date.now(),
        updated_at: Date.now(),
        last_opened_at: null,
      },
    ]);
    const subscribeRuntimeEvents = vi.spyOn(
      exagentClient,
      "subscribeRuntimeEvents",
    );
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "session-created",
      turn: {
        id: "turn-created",
        status: "in_progress",
        items: [],
      },
    });
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: null,
      composerValue: "Build the draft flow",
      sessions: [],
      transcript: [],
      events: [],
    });

    await useWorkbenchStore.getState().sendPrompt();

    expect(startThread).toHaveBeenCalledWith("project-exagent");
    expect(subscribeRuntimeEvents).toHaveBeenCalledWith(
      "project-exagent",
      "session-created",
      expect.any(Function),
    );
    expect(subscribeRuntimeEvents.mock.invocationCallOrder[0]).toBeLessThan(
      startTurn.mock.invocationCallOrder[0],
    );
    expect(startTurn).toHaveBeenCalledWith(
      "project-exagent",
      "session-created",
      "Build the draft flow",
      {
        model: null,
        thinkingMode: null,
        clearThinkingMode: false,
        turnMode: "default",
      },
    );
    expect(useWorkbenchStore.getState()).toMatchObject({
      activeSessionId: "session-created",
      composerValue: "",
      sessions: [
        {
          id: "session-created",
          title: "New session",
          status: "running",
        },
      ],
    });
  });

  it("aggregates tool invocation lifecycle events into one rendered tool card", async () => {
    render(<App />);

    await screen.findByLabelText("Message ExAgent");
    act(() => {
      useWorkbenchStore.setState({
        activeSessionId: "session-desktop",
        transcript: [],
        events: [],
        sessions: [
          {
            id: "session-desktop",
            projectId: "project-exagent",
            title: "Desktop GUI workbench",
            updatedAt: "local preview",
            status: "running",
          },
        ],
        loading: false,
      });
    });

    act(() => {
      const store = useWorkbenchStore.getState();
      store.applyRuntimeEvent({
        event_id: "evt-tool-started",
        thread_id: "session-desktop",
        turn_id: "turn-tool",
        kind: {
          type: "tool_invocation_started",
          invocation_id: "inv_call_1",
          tool_call_id: "call_1",
          tool_name: "run_command",
          mutating: true,
        },
      });
      store.applyRuntimeEvent({
        event_id: "evt-tool-delta-1",
        thread_id: "session-desktop",
        turn_id: "turn-tool",
        kind: {
          type: "tool_invocation_output_delta",
          invocation_id: "inv_call_1",
          stream: "stdout",
          chunk: "stdout: one\n",
          sequence: 1,
        },
      });
      store.applyRuntimeEvent({
        event_id: "evt-exec-output",
        thread_id: "session-desktop",
        turn_id: "turn-tool",
        kind: {
          type: "exec_output",
          exec_session_id: "exec_1",
          stream: "stdout",
          chunk: "legacy exec duplicate",
          sequence: 1,
        },
      });
      store.applyRuntimeEvent({
        event_id: "evt-tool-delta-2",
        thread_id: "session-desktop",
        turn_id: "turn-tool",
        kind: {
          type: "tool_invocation_output_delta",
          invocation_id: "inv_call_1",
          stream: "stdout",
          chunk: "two",
          sequence: 2,
        },
      });
      store.applyRuntimeEvent({
        event_id: "evt-tool-completed",
        thread_id: "session-desktop",
        turn_id: "turn-tool",
        kind: {
          type: "tool_invocation_completed",
          invocation_id: "inv_call_1",
          tool_call_id: "call_1",
          tool_name: "run_command",
          status: "success",
        },
      });
      store.applyRuntimeEvent({
        event_id: "evt-tool-result",
        thread_id: "session-desktop",
        turn_id: "turn-tool",
        kind: {
          type: "tool_result",
          result: {
            tool_call_id: "call_1",
            tool_name: "run_command",
            content: "stdout: one\ntwo",
            status: "success",
          },
        },
      });
    });

    const toolMessages = useWorkbenchStore
      .getState()
      .transcript.filter((message) => message.role === "tool");
    expect(toolMessages).toHaveLength(1);
    expect(toolMessages[0]).toMatchObject({
      invocationId: "inv_call_1",
      toolCallId: "call_1",
      title: "run_command",
      body: "stdout: one\ntwo",
      status: "success",
      toolStatus: "completed",
      mutating: true,
    });
    expect(
      useWorkbenchStore.getState().transcript.map((message) => message.body),
    ).not.toContain("legacy exec duplicate");
    expect(
      useWorkbenchStore
        .getState()
        .events.some((event) => event.id === "evt-tool-result"),
    ).toBe(true);

    expect(screen.getAllByText("run_command").length).toBeGreaterThan(0);
    expect(screen.getByText("Completed")).toBeInTheDocument();
    expect(screen.getByText("Mutating")).toBeInTheDocument();
    expect(
      screen.getByText(
        (_, element) => element?.textContent === "stdout: one\ntwo",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText("legacy exec duplicate")).not.toBeInTheDocument();
  });

  it("keeps live turn activity expanded until the assistant reply arrives", async () => {
    render(<App />);

    await screen.findByLabelText("Message ExAgent");
    act(() => {
      useWorkbenchStore.setState({
        activeSessionId: "session-desktop",
        transcript: [],
        events: [],
        sessions: [
          {
            id: "session-desktop",
            projectId: "project-exagent",
            title: "Desktop GUI workbench",
            updatedAt: "local preview",
            status: "running",
          },
        ],
        loading: false,
      });
    });

    act(() => {
      useWorkbenchStore.getState().applyRuntimeEvent({
        event_id: "evt-reasoning-delta-live",
        thread_id: "session-desktop",
        turn_id: "turn-live-activity",
        kind: {
          type: "reasoning_delta",
          delta: "checking files",
        },
      });
    });

    const liveGroup = screen.getByLabelText("Turn activity");
    const liveToggle = within(liveGroup).getByRole("button", { name: /Activity/ });
    expect(liveToggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("checking files")).toBeInTheDocument();

    act(() => {
      useWorkbenchStore.getState().applyRuntimeEvent({
        event_id: "evt-assistant-final-live",
        thread_id: "session-desktop",
        turn_id: "turn-live-activity",
        kind: {
          type: "assistant_turn",
          turn: {
            text: "Done.",
            tool_calls: [],
          },
        },
      });
    });

    await waitFor(() => {
      expect(liveToggle).toHaveAttribute("aria-expanded", "false");
    });
    expect(screen.getByText("Done.")).toBeInTheDocument();
    expect(screen.queryByText("checking files")).not.toBeInTheDocument();
  });

  it("streams reasoning and assistant deltas into stable transcript messages", async () => {
    render(<App />);

    await screen.findByLabelText("Message ExAgent");
    act(() => {
      useWorkbenchStore.setState({
        activeSessionId: "session-desktop",
        transcript: [],
        events: [],
        sessions: [
          {
            id: "session-desktop",
            projectId: "project-exagent",
            title: "Desktop GUI workbench",
            updatedAt: "local preview",
            status: "running",
          },
        ],
        loading: false,
      });
    });

    const applyStreamEvent = (event: BackendRuntimeEvent) => {
      act(() => {
        useWorkbenchStore.getState().applyRuntimeEvent(event);
      });
    };

    applyStreamEvent({
      event_id: "evt-reasoning-delta-1",
      thread_id: "session-desktop",
      turn_id: "turn-stream",
      kind: {
        type: "reasoning_delta",
        delta: "think ",
      },
    });
    applyStreamEvent({
      event_id: "evt-reasoning-delta-2",
      thread_id: "session-desktop",
      turn_id: "turn-stream",
      kind: {
        type: "reasoning_delta",
        delta: "first",
      },
    });
    applyStreamEvent({
      event_id: "evt-assistant-delta-1",
      thread_id: "session-desktop",
      turn_id: "turn-stream",
      kind: {
        type: "assistant_text_delta",
        delta: "hello ",
      },
    });
    applyStreamEvent({
      event_id: "evt-assistant-delta-2",
      thread_id: "session-desktop",
      turn_id: "turn-stream",
      kind: {
        type: "assistant_text_delta",
        delta: "world",
      },
    });

    expect(useWorkbenchStore.getState().transcript).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          role: "reasoning",
          body: "think first",
          turnId: "turn-stream",
        }),
        expect.objectContaining({
          role: "assistant",
          body: "hello world",
          turnId: "turn-stream",
        }),
      ]),
    );
    expect(screen.getByText("think first")).toBeInTheDocument();
    expect(screen.getByText("hello world")).toBeInTheDocument();

    act(() => {
      const store = useWorkbenchStore.getState();
      store.applyRuntimeEvent({
        event_id: "evt-reasoning-final",
        thread_id: "session-desktop",
        turn_id: "turn-stream",
        kind: {
          type: "reasoning",
          content: ["think first"],
        },
      });
      store.applyRuntimeEvent({
        event_id: "evt-assistant-final",
        thread_id: "session-desktop",
        turn_id: "turn-stream",
        kind: {
          type: "assistant_turn",
          turn: {
            text: "hello world",
            tool_calls: [],
          },
        },
      });
    });

    const reasoningMessages = useWorkbenchStore
      .getState()
      .transcript.filter((message) => message.role === "reasoning");
    const assistantMessages = useWorkbenchStore
      .getState()
      .transcript.filter((message) => message.role === "assistant");
    expect(reasoningMessages).toHaveLength(1);
    expect(assistantMessages).toHaveLength(1);
    expect(reasoningMessages[0]).toMatchObject({
      id: "evt-reasoning-final",
      body: "think first",
      turnId: "turn-stream",
    });
    expect(assistantMessages[0]).toMatchObject({
      id: "evt-assistant-final",
      body: "hello world",
      turnId: "turn-stream",
    });
    const streamActivityGroup = screen.getByLabelText("Turn activity");
    expect(within(streamActivityGroup).getByRole("button", { name: /Activity/ })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
    expect(screen.queryByText("think first")).not.toBeInTheDocument();
    expect(screen.getByText("hello world")).toBeInTheDocument();
  });

  it("opens history with tool invocation and tool result as one transcript card", async () => {
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-desktop",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [
          {
            id: "turn-history-tool",
            status: "completed",
            items: [
              {
                type: "tool_invocation",
                invocation_id: "inv_history_1",
                tool_call_id: "call_history_1",
                tool_name: "run_command",
                status: "completed",
                mutating: true,
                output_preview: "history output",
              },
              {
                type: "tool_result",
                name: "run_command",
              },
            ],
          },
        ],
      },
    });
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-desktop",
      events: [],
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(null);
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: null,
      transcript: [],
      events: [],
    });

    await useWorkbenchStore.getState().openSession("session-desktop");

    const toolMessages = useWorkbenchStore
      .getState()
      .transcript.filter((message) => message.role === "tool");
    expect(toolMessages).toHaveLength(1);
    expect(toolMessages[0]).toMatchObject({
      invocationId: "inv_history_1",
      toolCallId: "call_history_1",
      title: "run_command",
      body: "history output",
      toolStatus: "completed",
    });
  });

  it("keeps user prompts when rebuilding a transcript from thread history", async () => {
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-desktop",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [
          {
            id: "turn-history-chat",
            status: "completed",
            items: [
              {
                type: "user_message",
                text: "hi 介绍一下你自己吧",
              },
              {
                type: "assistant_message",
                event_id: "evt-history-assistant",
                text: "你好，我是 ExAgent。",
              },
            ],
          },
        ],
      },
    });
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-desktop",
      events: [],
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(null);
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: null,
      transcript: [],
      events: [],
    });

    await useWorkbenchStore.getState().openSession("session-desktop");

    expect(useWorkbenchStore.getState().transcript).toEqual([
      expect.objectContaining({
        role: "user",
        body: "hi 介绍一下你自己吧",
        turnId: "turn-history-chat",
      }),
      expect.objectContaining({
        role: "assistant",
        body: "你好，我是 ExAgent。",
        turnId: "turn-history-chat",
      }),
    ]);
  });

  it("keeps reasoning as a separate transcript item when rebuilding from thread history", async () => {
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-desktop",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [
          {
            id: "turn-history-reasoning",
            status: "completed",
            items: [
              {
                type: "reasoning",
                event_id: "evt-history-reasoning",
                summary: ["Checked the provider response shape."],
                content: ["raw provider reasoning"],
              },
              {
                type: "assistant_message",
                event_id: "evt-history-answer",
                text: "Final answer.",
              },
            ],
          },
        ],
      },
    });
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-desktop",
      events: [],
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(null);
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: null,
      transcript: [],
      events: [],
    });

    await useWorkbenchStore.getState().openSession("session-desktop");

    expect(useWorkbenchStore.getState().transcript).toEqual([
      expect.objectContaining({
        role: "reasoning",
        title: "Reasoning",
        body: "Checked the provider response shape.\n\nraw provider reasoning",
        turnId: "turn-history-reasoning",
      }),
      expect.objectContaining({
        role: "assistant",
        body: "Final answer.",
        turnId: "turn-history-reasoning",
      }),
    ]);
  });

  it("rebuilds historical reasoning when empty arrays were omitted by the backend", async () => {
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-desktop",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [
          {
            id: "turn-history-reasoning-omitted",
            status: "completed",
            items: [
              {
                type: "reasoning",
                event_id: "evt-history-reasoning-omitted",
                content: ["provider reasoning without summary"],
              },
              {
                type: "assistant_message",
                event_id: "evt-history-answer-omitted",
                text: "Final answer.",
              },
            ],
          },
        ],
      },
    });
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-desktop",
      events: [],
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(null);
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: null,
      transcript: [],
      events: [],
    });

    await useWorkbenchStore.getState().openSession("session-desktop");

    expect(useWorkbenchStore.getState().error).toBeNull();
    expect(useWorkbenchStore.getState().transcript).toEqual([
      expect.objectContaining({
        role: "reasoning",
        body: "provider reasoning without summary",
        turnId: "turn-history-reasoning-omitted",
      }),
      expect.objectContaining({
        role: "assistant",
        body: "Final answer.",
        turnId: "turn-history-reasoning-omitted",
      }),
    ]);
  });

  it("renders bare history tool results when a turn has no tool invocation", async () => {
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-desktop",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [
          {
            id: "turn-bare-tool-result",
            status: "completed",
            items: [
              {
                type: "tool_result",
                name: "legacy_tool",
              },
            ],
          },
          {
            id: "turn-tool-invocation-result",
            status: "completed",
            items: [
              {
                type: "tool_invocation",
                invocation_id: "inv_history_with_result",
                tool_call_id: "call_history_with_result",
                tool_name: "run_command",
                status: "completed",
                output_preview: "history output",
              },
              {
                type: "tool_result",
                name: "run_command",
              },
            ],
          },
        ],
      },
    });
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-desktop",
      events: [],
    });
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(null);
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: null,
      transcript: [],
      events: [],
    });

    await useWorkbenchStore.getState().openSession("session-desktop");

    const toolMessages = useWorkbenchStore
      .getState()
      .transcript.filter((message) => message.role === "tool");
    expect(toolMessages).toHaveLength(2);
    expect(toolMessages).toContainEqual(
      expect.objectContaining({
        title: "legacy_tool",
        body: "Tool completed.",
        turnId: "turn-bare-tool-result",
      }),
    );
    expect(
      toolMessages.filter(
        (message) => message.turnId === "turn-tool-invocation-result",
      ),
    ).toHaveLength(1);
  });

  it("keeps the mock open session path usable without runtime subscriptions", async () => {
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: null,
      transcript: [],
      events: [],
    });

    await useWorkbenchStore.getState().openSession("session-desktop");

    expect(useWorkbenchStore.getState()).toMatchObject({
      activeSessionId: "session-desktop",
      error: null,
      events: [],
    });
    expect(useWorkbenchStore.getState().transcript).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          body: expect.stringContaining("collapsible reveal"),
        }),
      ]),
    );
  });

  it("subscribes before replay and buffers live events while opening a session", async () => {
    const callOrder: string[] = [];
    let resolveReplay: (value: {
      thread_id: string;
      events: [];
    }) => void = () => {};
    const replayPromise = new Promise<{ thread_id: string; events: [] }>(
      (resolve) => {
        resolveReplay = resolve;
      },
    );
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockImplementation(
      async (_projectId, _threadId, onEvent) => {
        callOrder.push("subscribe");
        onEvent({
          event_id: "evt-buffered-tool-started",
          thread_id: "session-desktop",
          turn_id: "turn-buffered",
          kind: {
            type: "tool_invocation_started",
            invocation_id: "inv_buffered_1",
            tool_call_id: "call_buffered_1",
            tool_name: "run_command",
            mutating: false,
          },
        });
        return vi.fn();
      },
    );
    vi.spyOn(exagentClient, "resumeThread").mockImplementation(async () => {
      callOrder.push("resume");
      return {
        thread: {
          id: "session-desktop",
          status: "running",
          goal_mode: "standard",
          active_turn: null,
          turns: [],
        },
      };
    });
    vi.spyOn(exagentClient, "replayEvents").mockImplementation(async () => {
      callOrder.push("replay");
      return replayPromise;
    });
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: null,
      transcript: [],
      events: [],
    });

    const openPromise = useWorkbenchStore
      .getState()
      .openSession("session-desktop");
    await Promise.resolve();
    resolveReplay({
      thread_id: "session-desktop",
      events: [],
    });
    await openPromise;

    expect(callOrder.indexOf("subscribe")).toBeLessThan(
      callOrder.indexOf("replay"),
    );
    expect(useWorkbenchStore.getState().transcript).toContainEqual(
      expect.objectContaining({
        invocationId: "inv_buffered_1",
        toolCallId: "call_buffered_1",
        title: "run_command",
        toolStatus: "running",
      }),
    );
  });

  it("shows resumed transcript even when event subscription is slow", async () => {
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockImplementation(
      () => new Promise(() => undefined),
    );
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-slow-subscribe",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [
          {
            id: "turn-slow-subscribe",
            status: "completed",
            items: [
              { type: "assistant_message", text: "slow subscribe transcript" },
            ],
          },
        ],
      },
    });
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-slow-subscribe",
      events: [],
    });
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: "session-desktop",
      transcript: [
        {
          id: "old-message",
          role: "assistant",
          body: "old transcript",
          timestamp: "history",
          threadId: "session-desktop",
        },
      ],
      events: [],
    });

    void useWorkbenchStore.getState().openSession("session-slow-subscribe");
    await waitFor(() => {
      expect(
        useWorkbenchStore.getState().transcript.map((message) => message.body),
      ).toEqual(["slow subscribe transcript"]);
    });

    expect(useWorkbenchStore.getState().activeSessionId).toBe(
      "session-slow-subscribe",
    );
    expect(
      useWorkbenchStore.getState().transcript.map((message) => message.body),
    ).not.toContain("old transcript");
  });

  it("keeps session switches loading until the resumed transcript arrives", async () => {
    let resolveResume: (
      value: Awaited<ReturnType<typeof exagentClient.resumeThread>>,
    ) => void = () => {};
    const resume = new Promise<
      Awaited<ReturnType<typeof exagentClient.resumeThread>>
    >((resolve) => {
      resolveResume = resolve;
    });
    vi.spyOn(exagentClient, "resumeThread").mockReturnValue(resume);
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(null);
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-loading",
      events: [],
    });
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: "session-old",
      loading: false,
      transcript: [
        {
          id: "old-message",
          role: "assistant",
          body: "old transcript",
          timestamp: "history",
          threadId: "session-old",
        },
      ],
      events: [],
    });

    const open = useWorkbenchStore.getState().openSession("session-loading");
    await Promise.resolve();

    expect(useWorkbenchStore.getState().loading).toBe(true);
    expect(useWorkbenchStore.getState().transcript).toEqual([]);

    resolveResume({
      thread: {
        id: "session-loading",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [
          {
            id: "turn-loading",
            status: "completed",
            items: [{ type: "assistant_message", text: "loaded transcript" }],
          },
        ],
      },
    });
    await open;

    expect(useWorkbenchStore.getState().loading).toBe(false);
    expect(
      useWorkbenchStore.getState().transcript.map((message) => message.body),
    ).toEqual(["loaded transcript"]);
  });

  it("keeps overlapping openSession results scoped to the latest request", async () => {
    let resolveA: (
      value: Awaited<ReturnType<typeof exagentClient.resumeThread>>,
    ) => void = () => {};
    let resolveB: (
      value: Awaited<ReturnType<typeof exagentClient.resumeThread>>,
    ) => void = () => {};
    const readA = new Promise<
      Awaited<ReturnType<typeof exagentClient.resumeThread>>
    >((resolve) => {
      resolveA = resolve;
    });
    const readB = new Promise<
      Awaited<ReturnType<typeof exagentClient.resumeThread>>
    >((resolve) => {
      resolveB = resolve;
    });
    const unlistenA = vi.fn();
    const unlistenB = vi.fn();
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockImplementation(
      async (_projectId, threadId) => {
        return threadId === "session-a" ? unlistenA : unlistenB;
      },
    );
    vi.spyOn(exagentClient, "resumeThread").mockImplementation(
      async (_projectId, threadId) => {
        return threadId === "session-a" ? readA : readB;
      },
    );
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-desktop",
      events: [],
    });
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: null,
      transcript: [],
      events: [],
    });

    const openA = useWorkbenchStore.getState().openSession("session-a");
    await Promise.resolve();
    const openB = useWorkbenchStore.getState().openSession("session-b");
    await Promise.resolve();

    resolveB({
      thread: {
        id: "session-b",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [
          {
            id: "turn-b",
            status: "completed",
            items: [{ type: "assistant_message", text: "B transcript" }],
          },
        ],
      },
    });
    await openB;

    resolveA({
      thread: {
        id: "session-a",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [
          {
            id: "turn-a",
            status: "completed",
            items: [{ type: "assistant_message", text: "A transcript" }],
          },
        ],
      },
    });
    await openA;

    expect(useWorkbenchStore.getState().activeSessionId).toBe("session-b");
    expect(
      useWorkbenchStore.getState().transcript.map((message) => message.body),
    ).toEqual(["B transcript"]);
    expect(unlistenA).not.toHaveBeenCalled();
    expect(unlistenB).not.toHaveBeenCalled();
    useWorkbenchStore.getState().eventUnlisten?.();
    expect(unlistenB).toHaveBeenCalledTimes(1);
  });

  it("cleans up a new subscription when resume or replay fails during openSession", async () => {
    for (const failure of ["resume", "replay"] as const) {
      const unlisten = vi.fn();
      vi.restoreAllMocks();
      vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockResolvedValue(
        unlisten,
      );
      vi.spyOn(exagentClient, "resumeThread").mockImplementation(async () => {
        if (failure === "resume") {
          throw new Error("resume failed");
        }
        return {
          thread: {
            id: `session-${failure}`,
            status: "idle",
            goal_mode: "standard",
            active_turn: null,
            turns: [],
          },
        };
      });
      vi.spyOn(exagentClient, "replayEvents").mockImplementation(async () => {
        if (failure === "replay") {
          throw new Error("replay failed");
        }
        return {
          thread_id: `session-${failure}`,
          events: [],
        };
      });
      useWorkbenchStore.setState({
        ...useWorkbenchStore.getInitialState(),
        activeProjectId: "project-exagent",
        activeSessionId: null,
        transcript: [],
        events: [],
      });

      await useWorkbenchStore.getState().openSession(`session-${failure}`);

      expect(unlisten).toHaveBeenCalledTimes(failure === "resume" ? 0 : 1);
      expect(useWorkbenchStore.getState().eventUnlisten).toBeNull();
      expect(useWorkbenchStore.getState().loading).toBe(false);
      expect(useWorkbenchStore.getState().error).toBe(`${failure} failed`);
    }
  });

  it("renders open session errors in the chat surface", () => {
    useWorkbenchStore.setState({
      ...useWorkbenchStore.getInitialState(),
      projects: [
        {
          id: "project-exagent",
          name: "ExAgent",
          path: "/Volumes/EXEXEX/ExAgent",
          active: true,
        },
      ],
      sessions: [
        {
          id: "session-error",
          projectId: "project-exagent",
          title: "Broken session",
          updatedAt: "now",
          status: "idle",
        },
      ],
      activeProjectId: "project-exagent",
      activeSessionId: "session-error",
      transcript: [],
      loading: false,
      error: "thread resume failed",
    });

    render(<App />);

    expect(screen.getByText("thread resume failed")).toBeInTheDocument();
  });

  it("cleans up the Tauri listener when runtime subscription command fails", async () => {
    const unlisten = vi.fn();
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    tauriMocks.listen.mockResolvedValue(unlisten);
    tauriMocks.invoke.mockRejectedValue(new Error("subscribe failed"));

    await expect(
      exagentClient.subscribeRuntimeEvents(
        "project-exagent",
        "session-desktop",
        vi.fn(),
      ),
    ).rejects.toThrow("subscribe failed");

    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("sends a backend unsubscribe when runtime event cleanup runs", async () => {
    const unlisten = vi.fn();
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    tauriMocks.listen.mockResolvedValue(unlisten);
    tauriMocks.invoke.mockResolvedValue({});

    const cleanup = await exagentClient.subscribeRuntimeEvents(
      "project-exagent",
      "session-desktop",
      vi.fn(),
    );

    expect(tauriMocks.invoke).toHaveBeenCalledWith("events_subscribe", {
      projectId: "project-exagent",
      threadId: "session-desktop",
      afterEventId: null,
    });

    cleanup?.();

    await waitFor(() => {
      expect(tauriMocks.invoke).toHaveBeenCalledWith("events_unsubscribe", {
        projectId: "project-exagent",
        threadId: "session-desktop",
      });
    });
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("keeps buffered transcript events even when replay returns the same event id", async () => {
    const bufferedEvent = {
      event_id: "evt-buffered-replayed-tool-started",
      thread_id: "session-desktop",
      turn_id: "turn-buffered",
      kind: {
        type: "tool_invocation_started" as const,
        invocation_id: "inv_buffered_replayed_1",
        tool_call_id: "call_buffered_replayed_1",
        tool_name: "run_command",
        mutating: false,
      },
    };
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockImplementation(
      async (_projectId, _threadId, onEvent) => {
        onEvent(bufferedEvent);
        return vi.fn();
      },
    );
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-desktop",
        status: "running",
        goal_mode: "standard",
        active_turn: null,
        turns: [],
      },
    });
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-desktop",
      events: [bufferedEvent],
    });
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: null,
      transcript: [],
      events: [],
    });

    await useWorkbenchStore.getState().openSession("session-desktop");

    expect(useWorkbenchStore.getState().transcript).toContainEqual(
      expect.objectContaining({
        invocationId: "inv_buffered_replayed_1",
        toolCallId: "call_buffered_replayed_1",
        title: "run_command",
        toolStatus: "running",
      }),
    );
    expect(
      useWorkbenchStore
        .getState()
        .events.filter((event) => event.id === bufferedEvent.event_id),
    ).toHaveLength(1);
  });

  it("does not duplicate buffered assistant events already represented by the resumed thread view", async () => {
    const bufferedEvent = {
      event_id: "evt-buffered-replayed-assistant",
      thread_id: "session-desktop",
      turn_id: "turn-buffered-assistant",
      kind: {
        type: "assistant_turn" as const,
        turn: {
          text: "Buffered assistant answer",
          tool_calls: [],
        },
      },
    };
    vi.spyOn(exagentClient, "subscribeRuntimeEvents").mockImplementation(
      async (_projectId, _threadId, onEvent) => {
        onEvent(bufferedEvent);
        return vi.fn();
      },
    );
    vi.spyOn(exagentClient, "resumeThread").mockResolvedValue({
      thread: {
        id: "session-desktop",
        status: "idle",
        goal_mode: "standard",
        active_turn: null,
        turns: [
          {
            id: "turn-buffered-assistant",
            status: "completed",
            items: [
              {
                type: "assistant_message",
                event_id: "evt-buffered-replayed-assistant",
                text: "Buffered assistant answer",
              },
            ],
          },
        ],
      },
    });
    vi.spyOn(exagentClient, "replayEvents").mockResolvedValue({
      thread_id: "session-desktop",
      events: [bufferedEvent],
    });
    useWorkbenchStore.setState({
      activeProjectId: "project-exagent",
      activeSessionId: null,
      transcript: [],
      events: [],
    });

    await useWorkbenchStore.getState().openSession("session-desktop");

    const assistantMessages = useWorkbenchStore
      .getState()
      .transcript.filter(
        (message) =>
          message.role === "assistant" &&
          message.body === "Buffered assistant answer",
      );
    expect(assistantMessages).toHaveLength(1);
    expect(
      useWorkbenchStore
        .getState()
        .events.filter((event) => event.id === bufferedEvent.event_id),
    ).toHaveLength(1);
  });

  it("updates a waiting approval tool invocation when an approval decision arrives", () => {
    useWorkbenchStore.setState({
      activeSessionId: "session-desktop",
      transcript: [],
      events: [],
      sessions: [
        {
          id: "session-desktop",
          projectId: "project-exagent",
          title: "Desktop GUI workbench",
          updatedAt: "local preview",
          status: "awaiting_approval",
        },
      ],
      loading: false,
    });

    const store = useWorkbenchStore.getState();
    store.applyRuntimeEvent({
      event_id: "evt-waiting-approval",
      thread_id: "session-desktop",
      turn_id: "turn-approval",
      kind: {
        type: "tool_invocation_waiting_approval",
        invocation_id: "inv_needs_approval",
        approval_id: "approval_1",
        reason: "Needs permission",
      },
    });
    store.applyRuntimeEvent({
      event_id: "evt-approval-decision",
      thread_id: "session-desktop",
      turn_id: "turn-approval",
      kind: {
        type: "approval_decision",
        approval_id: "approval_1",
        status: "approved",
        note: "desktop approved",
      },
    });

    const approvalMessages = useWorkbenchStore
      .getState()
      .transcript.filter((message) => message.approvalId === "approval_1");
    expect(approvalMessages).toHaveLength(1);
    expect(approvalMessages[0]).toMatchObject({
      invocationId: "inv_needs_approval",
      approvalId: "approval_1",
      title: "Approval approved",
      body: "desktop approved",
      status: "success",
      toolStatus: "completed",
    });
  });

  it("returns a denied approval session to idle after the synthetic failed invocation", () => {
    useWorkbenchStore.setState({
      activeSessionId: "session-desktop",
      transcript: [],
      events: [],
      sessions: [
        {
          id: "session-desktop",
          projectId: "project-exagent",
          title: "Desktop GUI workbench",
          updatedAt: "local preview",
          status: "awaiting_approval",
        },
      ],
      loading: false,
    });

    const store = useWorkbenchStore.getState();
    store.applyRuntimeEvent({
      event_id: "evt-deny-waiting-approval",
      thread_id: "session-desktop",
      turn_id: "turn-deny-approval",
      kind: {
        type: "tool_invocation_waiting_approval",
        invocation_id: "inv_deny_approval",
        approval_id: "approval_deny",
        reason: "Needs permission",
      },
    });
    store.applyRuntimeEvent({
      event_id: "evt-deny-failed-invocation",
      thread_id: "session-desktop",
      turn_id: "turn-deny-approval",
      kind: {
        type: "tool_invocation_failed",
        invocation_id: "inv_approval_decision",
        tool_call_id: "approval_decision_approval_deny",
        tool_name: "run_command",
        message: "Approval denied",
      },
    });
    store.applyRuntimeEvent({
      event_id: "evt-deny-approval-decision",
      thread_id: "session-desktop",
      turn_id: "turn-deny-approval",
      kind: {
        type: "approval_decision",
        approval_id: "approval_deny",
        status: "denied",
        note: "desktop denied",
      },
    });

    expect(
      useWorkbenchStore
        .getState()
        .sessions.find((session) => session.id === "session-desktop")?.status,
    ).toBe("idle");
  });

  it("does not merge matching tool call ids across different turns", () => {
    useWorkbenchStore.setState({
      activeSessionId: "session-desktop",
      transcript: [],
      events: [],
      loading: false,
    });

    const store = useWorkbenchStore.getState();
    store.applyRuntimeEvent({
      event_id: "evt-same-call-turn-1",
      thread_id: "session-desktop",
      turn_id: "turn-1",
      kind: {
        type: "tool_invocation_started",
        invocation_id: "inv_turn_1",
        tool_call_id: "call_1",
        tool_name: "run_command",
        mutating: false,
      },
    });
    store.applyRuntimeEvent({
      event_id: "evt-same-call-turn-2",
      thread_id: "session-desktop",
      turn_id: "turn-2",
      kind: {
        type: "tool_invocation_started",
        invocation_id: "inv_turn_2",
        tool_call_id: "call_1",
        tool_name: "run_command",
        mutating: false,
      },
    });

    const toolMessages = useWorkbenchStore
      .getState()
      .transcript.filter((message) => message.role === "tool");
    expect(toolMessages).toHaveLength(2);
    expect(toolMessages.map((message) => message.turnId)).toEqual([
      "turn-1",
      "turn-2",
    ]);
  });

  it("keeps review-required tool results waiting until approval decision arrives", () => {
    useWorkbenchStore.setState({
      activeSessionId: "session-desktop",
      transcript: [],
      events: [],
      loading: false,
    });

    const store = useWorkbenchStore.getState();
    store.applyRuntimeEvent({
      event_id: "evt-review-started",
      thread_id: "session-desktop",
      turn_id: "turn-review-required",
      kind: {
        type: "tool_invocation_started",
        invocation_id: "inv_review_required",
        tool_call_id: "call_review_required",
        tool_name: "run_command",
        mutating: true,
      },
    });
    store.applyRuntimeEvent({
      event_id: "evt-review-waiting-approval",
      thread_id: "session-desktop",
      turn_id: "turn-review-required",
      kind: {
        type: "tool_invocation_waiting_approval",
        invocation_id: "inv_review_required",
        approval_id: "approval_review_required",
        reason: "Needs permission",
      },
    });
    store.applyRuntimeEvent({
      event_id: "evt-review-required-result",
      thread_id: "session-desktop",
      turn_id: "turn-review-required",
      kind: {
        type: "tool_result",
        result: {
          tool_call_id: "call_review_required",
          tool_name: "run_command",
          content: "Awaiting approval.",
          status: "review_required",
        },
      },
    });

    expect(
      useWorkbenchStore
        .getState()
        .transcript.find(
          (message) => message.invocationId === "inv_review_required",
        ),
    ).toMatchObject({
      approvalId: "approval_review_required",
      title: "Waiting for approval",
      body: "Needs permission",
      status: "warning",
      toolStatus: "waiting_approval",
    });

    store.applyRuntimeEvent({
      event_id: "evt-review-required-denied",
      thread_id: "session-desktop",
      turn_id: "turn-review-required",
      kind: {
        type: "approval_decision",
        approval_id: "approval_review_required",
        status: "denied",
        note: "desktop denied",
      },
    });

    expect(
      useWorkbenchStore
        .getState()
        .transcript.find(
          (message) => message.invocationId === "inv_review_required",
        ),
    ).toMatchObject({
      approvalId: "approval_review_required",
      title: "Approval denied",
      body: "desktop denied",
      status: "danger",
      toolStatus: "cancelled",
    });
  });

  it("lets waiting approval update a review-required placeholder in the same turn", () => {
    useWorkbenchStore.setState({
      activeSessionId: "session-desktop",
      transcript: [],
      events: [],
      loading: false,
    });

    const store = useWorkbenchStore.getState();
    store.applyRuntimeEvent({
      event_id: "evt-review-required-first",
      thread_id: "session-desktop",
      turn_id: "turn-review-before-approval",
      kind: {
        type: "tool_result",
        result: {
          tool_call_id: "call_review_before_approval",
          tool_name: "run_command",
          content: "Awaiting approval.",
          status: "review_required",
        },
      },
    });
    store.applyRuntimeEvent({
      event_id: "evt-waiting-after-review-required",
      thread_id: "session-desktop",
      turn_id: "turn-review-before-approval",
      kind: {
        type: "tool_invocation_waiting_approval",
        invocation_id: "inv_review_before_approval",
        approval_id: "approval_review_before_approval",
        reason: "Needs permission",
      },
    });

    expect(useWorkbenchStore.getState().transcript).toHaveLength(1);
    expect(useWorkbenchStore.getState().transcript[0]).toMatchObject({
      invocationId: "inv_review_before_approval",
      approvalId: "approval_review_before_approval",
      toolCallId: "call_review_before_approval",
      title: "Waiting for approval",
      body: "Needs permission",
      status: "warning",
      toolStatus: "waiting_approval",
    });
  });
});
