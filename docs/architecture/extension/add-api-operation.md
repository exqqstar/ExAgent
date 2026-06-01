# Add An API Operation

## Normal Change Set

1. Add request/response types in `src/app_server/protocol.rs`.
2. Add a method to `AppServerBoundary` in `src/app_server/service.rs`.
3. Implement the method through `ThreadManager`.
4. Add a route in `src/entrypoints/api.rs`.
5. Add CLI adapter support only if the CLI needs it.
6. Update [modules/app-server/](../modules/app-server/README.md) and any affected flow.

## Design Checklist

- Is this a thread operation, turn operation, event operation, or admin operation?
- Does it need a loaded runtime?
- Can it operate from persisted rollout only?
- What happens when the thread is busy?
- What HTTP status should each error map to?
