# kap-server Rust Migration Plan

## Source Dependency Structure

```text
index.ts (public exports)
`-- start.ts (composition root and server lifecycle)
    |-- agent-core-v2 (DI scopes and domain services)
    |-- middleware (request ID, auth, host/origin, rate limit, validation)
    |-- routes (REST /api/v1, OpenAPI, AsyncAPI, web assets)
    |-- transport
    |   |-- debug service dispatcher
    |   `-- WebSocket v1, connection registry, event journal/broadcast, FS watch
    `-- services
        |-- auth and persistent token storage
        |-- GUI TOML store
        |-- snapshots
        |-- transcripts
        |-- model catalog refresh
        `-- legacy status projection

protocol --> routes, transport, OpenAPI/AsyncAPI
security/lib/utilities --> middleware, services, start.ts
```

Workspace dependencies:

- `@moonshot-ai/agent-core-v2` -> `kimi-code-agent-core-v2`: scopes, sessions,
  agents, configuration, filesystem, terminal, tools, tasks, OAuth, models, and
  domain events.
- `@moonshot-ai/transcript` -> `kimi-code-transcript`: transcript storage,
  pagination, projections, subscriptions, and wire events.

External dependencies and intended Rust roles:

| TypeScript dependency | Role | Rust direction |
| --- | --- | --- |
| Fastify, multipart, Swagger | HTTP, routing, uploads, API documents | Tokio-based HTTP stack (prefer Axum/Tower) plus explicit compatibility tests |
| `ws` | WebSocket upgrade and frames | Axum WebSocket or Tokio Tungstenite |
| Zod | wire schemas and validation | Serde types plus explicit validation; generate documents only if output remains compatible |
| Pino | structured logging | `tracing`/`tracing-subscriber` adapter |
| `bcryptjs` | password verification | maintained bcrypt crate |
| `smol-toml` | GUI store format | `toml`, preserving formatting-relevant behavior where tested |
| `ulid` | request, instance, connection, and event IDs | `ulid` |

## Target Module Shape

Mirror the source boundaries under `src/`: `protocol`, `middleware`, `routes`,
`services`, `transport`, `security`, and `lib`. Use `lib.rs` for the public
surface and `start.rs` for `start_server` and `RunningServer`. Structural
changes are allowed only for ownership, typed errors, async boundaries, and
resource lifecycle, with source-mapping comments on significant methods.

## Translation Steps

1. **Freeze the compatibility baseline.** Inventory every public export, route,
   WebSocket message, schema, error code, persistent file, environment option,
   and lifecycle side effect. Capture reusable fixtures from the TypeScript
   tests, including OpenAPI/AsyncAPI and API-surface snapshots.
2. **Port protocol and pure utilities.** Translate envelopes, error codes,
   request IDs, pagination, REST/WS DTOs, bind classification, host/origin
   rules, bearer subprotocol parsing, and document transforms. Add JSON and
   byte-for-byte parity tests before higher layers depend on them.
3. **Port local services.** Migrate auth/token files, instance registry and
   heartbeat, GUI store, request logging, model refresh scheduling, snapshot
   reading, transcript binding/projection, and legacy status. Use Tokio for
   file I/O, timers, watching, and waiting; preserve atomic writes and cleanup.
4. **Port transport infrastructure.** Implement typed dispatch errors,
   connection/channel registries, in-flight tracking, event journal and
   broadcaster, filesystem watch bridge, buffering/backpressure, resync, and
   cancellation-safe shutdown.
5. **Port middleware.** Preserve hook order and exact behavior for request IDs,
   validation, envelopes, auth, rate limits, host/origin checks, security
   headers, and request logs.
6. **Port REST routes incrementally.** Register and verify one route module (or
   tightly coupled group) at a time: meta/auth/config/models; sessions and
   workspaces; messages/transcripts/prompts; approvals/questions; tools/tasks;
   terminals/filesystems/files; GUI store/connections/snapshots/export; then
   shutdown and debug routes. Keep agent-core calls and side-effect order
   identical.
7. **Port WebSocket v1.** Preserve upgrade path selection, host/origin/auth
   order, bearer protocol negotiation, handshake, subscriptions, event
   batching, high-water limits, replay/resync, close codes, and registry
   cleanup.
8. **Port server composition.** Implement `start_server`, dependency wiring,
   API documents, optional web assets, exposure policy, instance registration,
   port-plus-one retry, background task ownership, and ordered close/failure
   cleanup.
9. **Run end-to-end parity checks.** Reuse source vectors against both
   implementations and compare responses, errors, events, files, logs where
   observable, retry/timeout behavior, and shutdown. Finish with workspace
   format, check, test, and Clippy validation.
