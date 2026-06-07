# Security Policy

## Supported Versions

ExAgent is pre-1.0. Security fixes target the current `main` branch unless a
release branch is explicitly announced.

## Reporting a Vulnerability

Do not open a public issue with exploit details, credentials, OAuth tokens,
private rollout data, or reproduction steps that expose another system.

Preferred reporting path:

1. Use GitHub private vulnerability reporting for this repository if it is
   enabled.
2. If private reporting is not available, contact the maintainer out of band
   and share only a minimal summary until a private channel is established.

Please include:

- Affected commit, release, or branch.
- Impacted component, such as runtime tools, app-server API, desktop secret
  storage, provider auth, or persistence.
- Reproduction steps with test credentials or redacted data only.
- Any known mitigations.

## Scope

Security-sensitive areas include:

- command execution and approval policy
- workspace path validation
- rollout persistence and transcript replay
- desktop settings and secret storage
- provider credentials and OAuth tokens
- MCP and dynamic tool configuration

The current `full_access` permission profile is not an OS sandbox. Treat it as
a local development mode with tool-level checks only.
