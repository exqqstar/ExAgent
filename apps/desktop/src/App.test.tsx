import "@testing-library/jest-dom/vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "@/App";
import { exagentClient } from "@/api/exagentClient";

describe("AppShell", () => {
  it("renders the main desktop workbench regions", async () => {
    render(<App />);

    expect((await screen.findAllByText("Desktop GUI workbench"))[0]).toBeInTheDocument();
    expect(screen.getByText("Project")).toBeInTheDocument();
    expect(screen.getByText("Sessions")).toBeInTheDocument();
    expect(screen.getAllByText("Inspector")[0]).toBeInTheDocument();
    expect(screen.getByLabelText("Prompt composer")).toBeInTheDocument();
  });

  it("renders scaffold transcript labels and runtime notes", async () => {
    render(<App />);

    expect(await screen.findByText("Session restored")).toBeInTheDocument();
    expect(screen.getByText("Preview")).toBeInTheDocument();
    expect(screen.getByText("Changed Files")).toBeInTheDocument();
    expect(screen.getByLabelText("Message ExAgent")).toBeInTheDocument();
  });

  it("opens settings to the providers tab from the lower sidebar", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    await userEvent.click(screen.getByRole("button", { name: "Open settings" }));

    expect(screen.getByRole("heading", { name: "Settings" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Providers" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("No connected provider")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Configure OpenAI" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Configure Anthropic" })).toBeEnabled();
  });

  it("tests provider connections from settings before saving", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    await userEvent.click(screen.getByRole("button", { name: "Open settings" }));
    await userEvent.click(screen.getByRole("button", { name: "Configure OpenAI" }));
    await userEvent.click(screen.getByRole("button", { name: "Test connection" }));

    expect(await screen.findByText("Connection succeeded.")).toBeInTheDocument();
  });

  it("opens a provider-specific connection page from Configure", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    await userEvent.click(screen.getByRole("button", { name: "Open settings" }));
    await userEvent.click(screen.getByRole("button", { name: "Configure OpenAI Compatible" }));

    const dialog = screen.getByRole("dialog", { name: "连接 OpenAI Compatible" });
    expect(dialog).toHaveClass("max-w-[860px]");
    expect(within(dialog).getByTestId("provider-connection-body")).toHaveClass("max-w-[720px]");
    expect(within(dialog).getByRole("button", { name: "Back to providers" })).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Base URL")).toHaveValue("http://127.0.0.1:11434/v1");
  });

  it("shows OpenAI login method choices before API key fields", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    await userEvent.click(screen.getByRole("button", { name: "Open settings" }));
    await userEvent.click(screen.getByRole("button", { name: "Configure OpenAI" }));

    expect(screen.getByRole("dialog", { name: "连接 OpenAI" })).toBeInTheDocument();
    expect(screen.getByText("ChatGPT Pro/Plus (browser)")).toBeInTheDocument();
    expect(screen.getByText("ChatGPT Pro/Plus (headless)")).toBeInTheDocument();
    expect(screen.getByText("API 密钥")).toBeInTheDocument();
  });

  it("shows GitHub Copilot deployment choices", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    await userEvent.click(screen.getByRole("button", { name: "Open settings" }));
    await userEvent.click(screen.getByRole("button", { name: "Configure GitHub Copilot" }));

    expect(screen.getByRole("dialog", { name: "连接 GitHub Copilot" })).toBeInTheDocument();
    expect(screen.getByText("GitHub.com")).toBeInTheDocument();
    expect(screen.getByText("GitHub Enterprise")).toBeInTheDocument();
  });

  it("discovers models from provider settings and keeps manual entry available", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    await userEvent.click(screen.getByRole("button", { name: "Open settings" }));
    await userEvent.click(screen.getByRole("button", { name: "Configure OpenAI Compatible" }));
    await userEvent.click(screen.getByRole("button", { name: "Discover models" }));

    expect(await screen.findByRole("button", { name: "Use gpt-4.1-mini" })).toBeInTheDocument();
    expect(screen.getByLabelText("Model")).toBeEnabled();
  });

  it("shows model and thinking controls in the composer", async () => {
    render(<App />);

    await screen.findByText("Session restored");

    expect(screen.getByLabelText("Composer model")).toHaveValue("gpt-4.1");
    expect(screen.getByRole("button", { name: "Thinking auto" })).toHaveAttribute(
      "aria-pressed",
      "true"
    );
    expect(screen.getByRole("button", { name: "Thinking high" })).toBeInTheDocument();
  });

  it("shows runtime mcp and skills settings tabs", async () => {
    render(<App />);

    await screen.findByText("Session restored");
    await userEvent.click(screen.getByRole("button", { name: "Open settings" }));

    expect(screen.getByRole("tab", { name: "Providers" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Runtime" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "MCP" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Skills" })).toBeInTheDocument();

    await userEvent.click(screen.getByRole("tab", { name: "MCP" }));
    expect(screen.getByRole("button", { name: "Add MCP server" })).toBeInTheDocument();

    await userEvent.click(screen.getByRole("tab", { name: "Skills" }));
    expect(screen.getByRole("button", { name: "Add skill root" })).toBeInTheDocument();
  });

  it("shows runtime configuration in the inspector", async () => {
    render(<App />);

    await screen.findByText("Session restored");

    expect(screen.getByRole("heading", { name: "Runtime" })).toBeInTheDocument();
    expect(screen.getByText("gpt-4.1")).toBeInTheDocument();
    expect(screen.getByText("auto")).toBeInTheDocument();
    expect(screen.getByText("MCP servers")).toBeInTheDocument();
    expect(screen.getByText("Skill roots")).toBeInTheDocument();
  });

  it("uses the saved provider for prompts without reloading the workbench", async () => {
    const user = userEvent.setup();
    const startTurn = vi.spyOn(exagentClient, "startTurn").mockResolvedValue({
      thread_id: "session-desktop",
      turn: {
        id: "turn-provider-switch",
        status: "in_progress",
        items: []
      }
    });
    render(<App />);

    await screen.findByText("Session restored");
    await user.click(screen.getByRole("button", { name: "Open settings" }));
    await user.click(screen.getByRole("button", { name: "Configure OpenAI Compatible" }));
    await user.click(screen.getByRole("button", { name: "提交" }));
    await user.keyboard("{Escape}");
    await user.type(screen.getByLabelText("Message ExAgent"), "Use the new provider");
    await user.click(screen.getByRole("button", { name: "Send message" }));

    expect(startTurn).toHaveBeenCalledWith(
      "project-exagent",
      "session-desktop",
      "Use the new provider",
      {
        provider_id: "openai_compatible",
        model_id: "local-model"
      },
      "auto"
    );
  });
});
