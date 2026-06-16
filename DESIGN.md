# Design

## Visual Direction

ExAgent Desktop is a product UI, not a marketing surface. The visual direction combines a local agent workbench structure, dense and quiet product craft, and macOS native restraint.

Physical scene: a developer is working late in a local project, with an editor, terminal, and agent workbench open side by side. The app should feel calm, legible, and precise under that ambient light.

## Color Strategy

Use a restrained neutral product palette across dark and light mode. Neutral surfaces carry most of the interface. Light mode should read as a soft gray project sidebar transitioning into a white main conversation canvas; dark mode should keep the same surface hierarchy without a hard black split. The primary accent is an action/status color, not a brand wash: use it sparingly for primary actions, send/ready states, and focus, while selection states stay mostly neutral. Blue, amber, and red are reserved for info, warning, and danger semantics.

Use OKLCH tokens only.

```css
:root {
  --color-bg: oklch(0.110 0.006 260);
  --color-surface-1: oklch(0.155 0.007 260);
  --color-surface-2: oklch(0.205 0.008 260);
  --color-surface-3: oklch(0.275 0.009 260);
  --color-border: oklch(0.325 0.010 260);
  --color-border-strong: oklch(0.455 0.014 260);

  --color-ink: oklch(0.930 0.004 260);
  --color-muted: oklch(0.735 0.010 260);
  --color-subtle: oklch(0.605 0.010 260);

  --color-primary: oklch(0.500 0.055 145);
  --color-primary-hover: oklch(0.455 0.065 145);
  --color-primary-muted: oklch(0.942 0.018 145);

  --color-info: oklch(0.760 0.110 245);
  --color-warning: oklch(0.760 0.140 75);
  --color-danger: oklch(0.620 0.160 28);
  --color-success: oklch(0.500 0.070 145);

  --color-focus: oklch(0.760 0.110 245);
}
```

Rules:

- Body text must meet at least 4.5:1 contrast against its surface.
- Primary color is not decoration. Use it for primary actions, focus, and focused agent states.
- Inactive UI should stay neutral. Do not tint every card or panel.
- Current sidebar selections use clear neutral backgrounds and subtle rings, not colored side stripes.
- Use semantic color with text labels or icons, never color alone.

## Typography

Use one product sans stack plus one mono stack.

```css
:root {
  --font-ui: -apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI", "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei UI", "Noto Sans SC", Inter, ui-sans-serif, system-ui, sans-serif;
  --font-mono: "SF Mono", "SFMono-Regular", ui-monospace, Menlo, Monaco, Consolas, monospace;
}
```

Type scale:

- `type-empty-title`: 28px / 500 / 36px. Use only for the new-session prompt headline.
- `type-title-lg`: 18px / 600 / 24px. Use for settings headers and provider names.
- `type-title-md`: 16px / 600 / 22px. Use for compact panel titles and primary sidebar/project names.
- `type-title-sm`: 15px / 600 / 20px. Use for inspector titles and approval headings.
- `type-body-lg`: 16px / 400 / 1.55. Use for transcript prose and primary textareas.
- `type-body-md`: 14px / 400 / 22px. Use for default UI body text, descriptions, and form copy.
- `type-body-sm`: 13px / 400 / 20px. Use for dense sidebar/list rows.
- `type-label-md`: 13px / 500 / 18px. Use for buttons, controls, row titles, and inspector labels.
- `type-label-sm`: 12px / 500 / 16px. Use for metadata, timestamps, compact group labels, and secondary row details.
- `type-code-sm`: 12px mono / 400 / 20px. Use for paths, model IDs, tool output, and aligned numbers.
- `type-badge`: 11px / 500 / 1. Use only inside badges.

Rules:

- No fluid type scaling for product UI.
- No display fonts in labels, buttons, tables, or inspector content.
- Prefer native system typography over generic web-font personality. Chinese fallback must be explicit so mixed English/Chinese UI keeps stable weight and rhythm.
- Use the semantic typography utilities before writing raw `text-*`, `font-*`, or arbitrary `text-[...]` classes. One-off sizes require a local reason.
- Keep transcript prose readable with `line-height: 1.55`.

## Layout

Primary layout:

```text
sidebar 280px | chat minmax(0, 1fr) | inspector 320px
```

Responsive behavior:

- `>= 1200px`: sidebar, chat, and inspector are visible.
- `900px-1199px`: inspector collapses into a drawer.
- `< 900px`: sidebar collapses, chat takes priority.

Spacing:

- Use a 4px base rhythm.
- Common gaps: `6px`, `8px`, `12px`, `16px`, `24px`.
- Dense lists use `6px-8px` vertical padding.
- Transcript blocks use `12px-16px` internal padding.

## Components

## Component Source System

Use shadcn/ui as the React component source system for the desktop frontend.
This does not define ExAgent's visual style by itself. ExAgent's style remains
the local workbench, dense product UI, and macOS native direction described in
this file.

Rules:

- Use shadcn/ui components as editable source under `apps/desktop/src/components/ui`.
- Use the `new-york` style with Radix primitives.
- Use Tailwind theme tokens and CSS variables rather than ad hoc hex colors.
- Keep shadcn components low-level. Product components live under `apps/desktop/src/components/workbench` or `apps/desktop/src/components`.
- Do not import shadcn blocks or templates as page designs.
- Do not let default shadcn SaaS dashboard aesthetics override this design baseline.

Buttons:

- Icon buttons use lucide icons with tooltips.
- Text buttons use verb-object labels.
- Primary buttons use the primary color sparingly.
- Disabled buttons must look disabled and remain readable.

Inputs:

- Prompt composer is the largest input in the app.
- Search inputs are compact and live in the sidebar.
- Focus rings use `--color-focus`.

Cards and blocks:

- Dense cards and list rows should stay compact, usually `8px-12px` radius.
- The prompt composer, main workspace transition, and modal-scale surfaces may use `16px` radius when it makes the desktop UI feel more native and less brittle.
- Do not nest cards inside cards.
- Tool output uses expandable blocks, not decorative cards.

Sidebar:

- Project and session rows are compact.
- Current selection uses a clear background and left alignment, not a thick colored side stripe.
- Pin and archive actions appear on hover or context menu.

Inspector:

- Inspector is for state, not explanation.
- Use compact labels and stable sections: Progress, Environment, Token Usage, Changed Files, Events.

Settings and provider connection pages:

- Settings dialogs should feel like a centered macOS utility window, not a full-page route.
- Provider connection dialogs use a maximum outer width of about `860px` and a centered content column around `720px`.
- Provider connection pages must share the same shell: back control, provider mark, title, description, body, and action area.
- Keep vertical spacing compact enough that primary actions remain comfortably inside the dialog on a 1024x600 CSS viewport.
- API key, OAuth, deployment-type, and endpoint/model flows should reuse the same connection shell instead of inventing per-provider page geometry.

Approvals:

- Approval cards must be inline with the active turn.
- Approve, Deny, and Interrupt actions must have explicit labels.
- Dangerous actions use semantic danger styling and plain-language consequences.

## Motion

Motion is functional and brief.

- Use 150-200ms transitions for hover, drawer, disclosure, and selected-row changes.
- Do not animate page load sequences.
- Do not animate layout properties when opacity or transform can communicate the state.
- Respect `prefers-reduced-motion: reduce`.

## Bans

- No gradient text.
- No decorative gradient blobs, bokeh, or orbs.
- No glassmorphism as the default surface.
- No ballooned, pill-like cards or inputs.
- No broad decorative shadows on bordered cards.
- No landing-page hero sections in the app shell.
- No custom scrollbars unless native behavior is insufficient.
