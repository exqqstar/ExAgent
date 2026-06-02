# ExAgent Architecture Notes

This directory is the living architecture map for ExAgent.

Older date-prefixed architecture notes in this directory are historical records. The structured folders below are the maintained entry points for understanding and extending the project.

## How To Read This

Start here if you feel lost:

1. Read [maps/module-map.md](maps/module-map.md) for the system shape.
2. Read [flows/turn-lifecycle.md](flows/turn-lifecycle.md) for the main request path.
3. Read the relevant page under [modules/](modules/README.md) when you need ownership and file details.
4. Read [maps/state-map.md](maps/state-map.md) when behavior depends on state.
5. Read [adr/](adr/README.md) when you need to know why a design exists.

## Directory Roles

- [maps/](maps/README.md): stable orientation maps for modules, files, and state.
- [flows/](flows/README.md): runtime stories such as turn execution, tool calls, event replay, and approvals.
- [modules/](modules/README.md): module cards, file maps, owned state, and extension points.
- [extension/](extension/README.md): practical guides for adding tools, events, and API operations.
- [benchmarks/](benchmarks/README.md): benchmark-driven architecture feedback and optimization follow-ups.
- [adr/](adr/README.md): architecture decision records. ADRs explain important design choices, not every file.

## Maintenance Rule

Use this split when updating docs:

- Module responsibility changed: update `modules/`.
- Flow or lifecycle changed: update `flows/`.
- File ownership or state ownership changed: update `maps/`.
- A durable design decision changed: add or supersede an ADR in `adr/`.
- A common extension path changed: update `extension/`.
