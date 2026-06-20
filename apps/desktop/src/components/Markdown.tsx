import { Check, Copy } from "lucide-react";
import { memo, useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import rehypeHighlight from "rehype-highlight";
import remarkGfm from "remark-gfm";
import { cn } from "@/lib/utils";

type HastNode = {
  type?: string;
  value?: string;
  tagName?: string;
  properties?: { className?: unknown };
  children?: HastNode[];
};

function hastText(node?: HastNode): string {
  if (!node) return "";
  if (node.type === "text") return node.value ?? "";
  return (node.children ?? []).map(hastText).join("");
}

function codeLanguage(node?: HastNode): string | undefined {
  const codeNode = node?.children?.find((child) => child.tagName === "code");
  const classes = codeNode?.properties?.className;
  const list = Array.isArray(classes) ? classes.map(String) : [];
  return list.find((name) => name.startsWith("language-"))?.slice("language-".length);
}

async function copyText(text: string) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "");
  textarea.style.position = "fixed";
  textarea.style.top = "-1000px";
  document.body.appendChild(textarea);
  textarea.select();
  document.execCommand("copy");
  textarea.remove();
}

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const id = window.setTimeout(() => setCopied(false), 1400);
    return () => window.clearTimeout(id);
  }, [copied]);
  return (
    <button
      type="button"
      onClick={() => {
        copyText(text)
          .then(() => setCopied(true))
          .catch(() => {});
      }}
      className="rounded bg-surface-2/80 p-1 text-subtle transition-colors hover:text-ink"
      aria-label={copied ? "Copied" : "Copy code"}
    >
      {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
    </button>
  );
}

// Horizontal-scroll wrapper that fades the edge(s) that have more content,
// so wide tables/code blocks visibly afford scrolling. ResizeObserver is
// guarded for non-DOM environments (e.g. jsdom in tests).
function HorizontalScroller({ children, className }: { children: ReactNode; className?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const [edges, setEdges] = useState({ left: false, right: false });

  const update = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    const left = el.scrollLeft > 1;
    const right = Math.ceil(el.scrollLeft + el.clientWidth) < el.scrollWidth - 1;
    setEdges((prev) => (prev.left === left && prev.right === right ? prev : { left, right }));
  }, []);

  useEffect(() => {
    update();
    const el = ref.current;
    window.addEventListener("resize", update);
    let observer: ResizeObserver | undefined;
    if (el && typeof ResizeObserver !== "undefined") {
      observer = new ResizeObserver(update);
      observer.observe(el);
    }
    return () => {
      window.removeEventListener("resize", update);
      observer?.disconnect();
    };
  }, [update]);

  const fade = "1.5rem";
  const maskImage = `linear-gradient(to right, transparent 0, #000 ${
    edges.left ? fade : "0px"
  }, #000 calc(100% - ${edges.right ? fade : "0px"}), transparent 100%)`;
  const masked = edges.left || edges.right;

  return (
    <div
      ref={ref}
      onScroll={update}
      className={cn("overflow-x-auto", className)}
      style={masked ? { maskImage, WebkitMaskImage: maskImage } : undefined}
    >
      {children}
    </div>
  );
}

function CodeBlock({ node, children }: { node?: HastNode; children?: ReactNode }) {
  const language = codeLanguage(node);
  const raw = hastText(node).replace(/\n$/, "");
  return (
    <div className="group/code relative message-card overflow-hidden rounded-lg border border-border">
      <div className="absolute right-2 top-2 z-10 flex items-center gap-1.5 opacity-0 transition-opacity group-hover/code:opacity-100">
        {language ? (
          <span className="type-label-sm rounded bg-surface-2/80 px-1.5 py-0.5 text-subtle">
            {language}
          </span>
        ) : null}
        <CopyButton text={raw} />
      </div>
      <HorizontalScroller>
        <pre className="type-code-sm w-fit min-w-full px-3 py-2 text-muted">{children}</pre>
      </HorizontalScroller>
    </div>
  );
}

const components: Components = {
  h1: ({ children }) => <h1 className="type-title-lg font-semibold text-ink">{children}</h1>,
  h2: ({ children }) => <h2 className="type-title-md font-semibold text-ink">{children}</h2>,
  h3: ({ children }) => <h3 className="type-title-sm font-semibold text-ink">{children}</h3>,
  h4: ({ children }) => <h4 className="type-title-sm font-semibold text-ink">{children}</h4>,
  h5: ({ children }) => <h5 className="type-label-md font-semibold text-ink">{children}</h5>,
  h6: ({ children }) => <h6 className="type-label-md font-semibold text-muted">{children}</h6>,
  p: ({ children }) => <p className="leading-relaxed break-words">{children}</p>,
  a: ({ href, children }) => (
    <a
      href={href}
      target="_blank"
      rel="noreferrer"
      className="text-ink underline underline-offset-2 transition-colors hover:text-ink/80"
    >
      {children}
    </a>
  ),
  img: ({ src, alt }) => (
    <img
      src={typeof src === "string" ? src : undefined}
      alt={alt ?? ""}
      loading="lazy"
      className="my-1 max-w-full rounded-lg border border-border"
    />
  ),
  strong: ({ children }) => <strong className="font-semibold text-ink">{children}</strong>,
  em: ({ children }) => <em className="italic">{children}</em>,
  ul: ({ children, className }) => (
    <ul
      className={cn(
        "space-y-1",
        className?.includes("contains-task-list") ? "list-none pl-1" : "list-disc pl-5"
      )}
    >
      {children}
    </ul>
  ),
  ol: ({ children }) => <ol className="list-decimal space-y-1 pl-5">{children}</ol>,
  li: ({ children, className }) => (
    <li className={cn("leading-relaxed", className?.includes("task-list-item") && "list-none")}>
      {children}
    </li>
  ),
  input: ({ type, checked }) =>
    type === "checkbox" ? (
      <input
        type="checkbox"
        checked={Boolean(checked)}
        readOnly
        className="mr-1.5 h-3.5 w-3.5 translate-y-[2px] accent-[var(--color-ink)]"
      />
    ) : null,
  blockquote: ({ children }) => (
    <blockquote className="border-l-2 border-border pl-4 italic text-muted">{children}</blockquote>
  ),
  hr: () => <hr className="border-border" />,
  pre: CodeBlock,
  code: ({ className, children }) => {
    const text = String(children ?? "");
    const isBlock = /\blanguage-|\bhljs\b/.test(className ?? "") || text.includes("\n");
    if (isBlock) {
      return <code className={cn("font-mono", className)}>{children}</code>;
    }
    return (
      <code className="rounded bg-surface-2 px-1 py-0.5 font-mono text-[0.92em] text-ink break-words">
        {children}
      </code>
    );
  },
  table: ({ children }) => (
    <HorizontalScroller>
      <table className="type-body-sm w-full border-collapse overflow-hidden rounded-lg border border-border">
        {children}
      </table>
    </HorizontalScroller>
  ),
  thead: ({ children }) => (
    <thead className="bg-surface-2 [&>tr]:border-b [&>tr]:border-border">{children}</thead>
  ),
  tbody: ({ children }) => <tbody className="[&>tr:last-child]:border-b-0">{children}</tbody>,
  tr: ({ children }) => <tr className="border-b border-border/60">{children}</tr>,
  th: ({ children, style }) => (
    <th style={style} className="px-3 py-1.5 text-left font-semibold text-ink">
      {children}
    </th>
  ),
  td: ({ children, style }) => (
    <td style={style} className="px-3 py-1.5 align-top text-muted break-words">
      {children}
    </td>
  )
};

const remarkPlugins = [remarkGfm];
const rehypePlugins = [[rehypeHighlight, { detect: false, ignoreMissing: true }]] as const;

// While a message streams in token-by-token, a code fence (```) can be open
// without its closing fence yet, which would render the rest of the message as
// a code block and flicker. Append a temporary closing fence to keep it stable.
function balanceFences(src: string): string {
  const open = (src.match(/^```/gm) ?? []).length;
  return open % 2 === 1 ? `${src}\n\`\`\`` : src;
}

export const Markdown = memo(function Markdown({
  content,
  className
}: {
  content: string;
  className?: string;
}) {
  return (
    <div className={cn("space-y-4", className)}>
      <ReactMarkdown
        remarkPlugins={remarkPlugins}
        rehypePlugins={rehypePlugins as never}
        components={components}
      >
        {balanceFences(content)}
      </ReactMarkdown>
    </div>
  );
});
