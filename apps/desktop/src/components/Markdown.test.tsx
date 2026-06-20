import "@testing-library/jest-dom/vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Markdown } from "@/components/Markdown";

describe("Markdown", () => {
  it("renders GFM tables as real table elements with aligned cells", () => {
    const md = ["| 维度 | Core | Pixel |", "|------|:----:|------:|", "| 字体 | 系统 | 像素 |"].join(
      "\n"
    );
    render(<Markdown content={md} />);

    const table = screen.getByRole("table");
    expect(table).toBeInTheDocument();
    expect(screen.getAllByRole("columnheader")).toHaveLength(3);
    expect(screen.getByRole("cell", { name: "系统" })).toBeInTheDocument();
    // The centered column's alignment is carried through from the separator row.
    expect(screen.getByRole("columnheader", { name: "Core" })).toHaveStyle({
      textAlign: "center"
    });
  });

  it("renders atx headings as heading elements (not raw ### text)", () => {
    render(<Markdown content={"### 代码复杂度"} />);
    expect(screen.getByRole("heading", { level: 3, name: "代码复杂度" })).toBeInTheDocument();
  });

  it("highlights fenced code blocks", () => {
    const { container } = render(<Markdown content={"```ts\nconst x = 1;\n```"} />);
    expect(container.querySelector("pre code.hljs")).not.toBeNull();
  });

  it("renders task lists with checkboxes", () => {
    render(<Markdown content={"- [x] done\n- [ ] todo"} />);
    const boxes = screen.getAllByRole("checkbox");
    expect(boxes).toHaveLength(2);
    expect(boxes[0]).toBeChecked();
    expect(boxes[1]).not.toBeChecked();
  });

  it("shows the language label and copies the raw code", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });

    render(<Markdown content={"```ts\nconst x = 1;\n```"} />);
    expect(screen.getByText("ts")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /copy code/i }));
    expect(writeText).toHaveBeenCalledWith("const x = 1;");
  });

  it("renders markdown images with constrained, lazy-loaded styling", () => {
    render(<Markdown content={"![alt text](https://example.com/a.png)"} />);
    const img = screen.getByRole("img", { name: "alt text" });
    expect(img).toHaveAttribute("loading", "lazy");
    expect(img).toHaveClass("max-w-full");
  });

  it("keeps an unclosed code fence from swallowing the rest while streaming", () => {
    // Mid-stream: opening fence with no closing fence yet.
    const { container } = render(<Markdown content={"intro\n\n```js\nconst x = 1;"} />);
    // Should still render a <pre> rather than leaving raw text dangling.
    expect(container.querySelector("pre")).not.toBeNull();
  });
});
