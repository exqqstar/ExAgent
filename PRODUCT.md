# Product

## Register

product

## Users

ExAgent is used by developers and agent builders working inside local code projects. They are usually in an editor, terminal, or desktop workbench, and they need to inspect runtime behavior while still making forward progress on real implementation tasks.

The desktop user is technical. They care about speed, recoverability, command visibility, approvals, thread history, and confidence that the agent is operating inside the intended project folder.

## Product Purpose

ExAgent Desktop is a local agent workbench for project-scoped conversations. It lets users choose a folder as a project, resume previous sessions, start new turns, observe tool execution, handle approvals, and inspect runtime state without running a separate HTTP server.

Success means the desktop app feels immediately usable for daily ExAgent work: project history is easy to scan, tool activity is visible without being noisy, and runtime state is understandable without reading rollout files by hand.

## Brand Personality

Quiet, exact, native.

The interface should feel like serious local software: calm enough for long work sessions, precise enough for runtime debugging, and native enough that it belongs next to an editor, terminal, and Finder window.

## References

- Local agent workbenches: project/session structure, agent event visibility, approval flow placement.
- Dense product tools: polished information hierarchy, restrained color, predictable controls.
- macOS native apps: window behavior, sidebar rhythm, toolbar restraint, system typography, familiar local-file affordances.

## Anti-references

- AI SaaS landing pages with purple-blue gradients, oversized heroes, glass cards, and decorative blobs.
- Chat-only clients that hide command execution, approvals, workspace, or runtime state.
- IDE clones with heavy file trees, embedded editors, and dense panels before the core agent workflow is stable.
- Over-rounded cards, nested cards, large decorative shadows, custom scrollbars, and novelty controls.
- Any design where the user cannot tell which project, thread, cwd, policy, or turn state is active.

## Design Principles

1. Project first: the selected folder and its sessions are the user's anchor.
2. Runtime visible, not loud: show tool execution, approvals, status, and changed files in the workflow without flooding the transcript.
3. Familiar product grammar: standard sidebars, buttons, inputs, menus, drawers, tabs, tooltips, and native file picking.
4. One source of truth: UI state can index and summarize, but runtime history remains in rollout.
5. Earned density: make repeated work fast to scan, not sparse for presentation.

## Accessibility & Inclusion

Target WCAG 2.2 AA for contrast, keyboard navigation, focus visibility, and screen-reader labels. Reduced motion must be supported. Color cannot be the only way to distinguish status, approval, success, warning, or error states.
