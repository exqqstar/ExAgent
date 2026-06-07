# Contributing

Thanks for taking the time to improve ExAgent. This project is still early, so
small, focused changes are easier to review than broad rewrites.

## Development Setup

Install the Rust toolchain and Node.js, then install the desktop dependencies:

```bash
cd apps/desktop
npm ci
```

The runtime binary reads environment variables directly. Use `.env.example` as
a reference and export values in your shell before running model-backed flows.
Do not commit real API keys, OAuth tokens, rollout files, benchmark outputs, or
local workspace state.

## Common Commands

From the repository root:

```bash
cargo fmt --all -- --check
cargo test --package exagent --locked
```

For the desktop frontend:

```bash
cd apps/desktop
npm test
npm run build
```

For the desktop Tauri shell:

```bash
cargo test --package exagent-desktop --locked
```

## Pull Requests

Before opening a pull request:

- Keep the scope narrow and describe the user-visible behavior change.
- Add or update tests when behavior changes.
- Update README or protocol docs when commands, API contracts, or setup steps
  change.
- Include known limitations when a change intentionally does not cover a case.
- Confirm the commands above were run, or explain why a command could not run.

## Architecture Changes

For changes to runtime persistence, thread lifecycle, app-server protocol,
tool execution, provider adapters, or desktop settings storage, describe the
rationale and trade-offs in the pull request. Maintainers may keep private
design notes outside the public repository when the details are not needed by
contributors.

## Security

Please do not report vulnerabilities in public issues. Follow `SECURITY.md`.

## License

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project is licensed as `MIT OR Apache-2.0`, without any
additional terms or conditions.
