import "@testing-library/jest-dom/vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { QuestionCard } from "@/components/QuestionCard";
import { submitUserInput } from "@/stores/workbenchStore";
import type { TranscriptMessage } from "@/types";

vi.mock("@/stores/workbenchStore", () => ({
  submitUserInput: vi.fn(),
}));

describe("QuestionCard", () => {
  beforeEach(() => {
    vi.mocked(submitUserInput).mockReset();
  });

  it("submits selected single-choice and multi-select options", async () => {
    const user = userEvent.setup();
    const message: TranscriptMessage = {
      id: "question-1",
      role: "tool",
      body: "",
      timestamp: "now",
      requestId: "req_1",
      toolStatus: "waiting_user_input",
      questions: [
        {
          question: "Pick one",
          options: [{ label: "A" }, { label: "B" }],
        },
        {
          question: "Pick many",
          multi_select: true,
          options: [{ label: "X" }, { label: "Y" }],
        },
      ],
    };

    render(<QuestionCard message={message} />);

    await user.click(screen.getByRole("radio", { name: "B" }));
    await user.click(screen.getByRole("checkbox", { name: "X" }));
    await user.click(screen.getByRole("checkbox", { name: "Y" }));
    await user.click(screen.getByRole("button", { name: "Submit" }));

    expect(submitUserInput).toHaveBeenCalledWith(message, [["B"], ["X", "Y"]], false);
  });
});
