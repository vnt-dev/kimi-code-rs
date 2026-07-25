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

## Implemented Units

The following source units now have tested Rust counterparts:

| TypeScript source | Rust counterpart | Notes |
| --- | --- | --- |
| `security/bindClassify.ts` | `src/security/bind_classify.rs` | Complete |
| `middleware/hostnames.ts`, `origin.ts` | `src/middleware/hostnames.rs`, `origin.rs`, `src/web/middleware.rs` | Predicates and Axum request-boundary wiring complete |
| `middleware/auth.ts`, `rateLimit.ts`, `securityHeaders.ts` | matching `src/middleware/*.rs`, `src/web/middleware.rs` | Policy and HTTP hook wiring complete |
| `services/auth/*` | `src/services/auth/*` | Complete, including live token rotation |
| `instanceRegistry.ts` | `src/instance_registry.rs` | Complete for explicit home/instances directories |
| `services/guiStore/guiStoreService.ts` | `src/services/gui_store/service.rs` | Complete; core-v2 DI decorator registration deferred |
| `services/snapshot/snapshotConfig.ts` | `src/services/snapshot/config.rs` | Complete |
| `snapshotReader.readWireRecords`, blob resolution | `src/services/snapshot/reader.rs` | Complete |
| `transport/ws/bearerProtocol.ts` | `src/transport/ws/bearer_protocol.rs` | Complete |
| `transport/ws/connectionRegistry.ts` | `src/transport/ws/connection_registry.rs` | Complete |
| `transport/ws/v1/inFlightTurnTracker.ts` | `src/transport/ws/v1/in_flight_turn_tracker.rs` | Complete, including JS UTF-16 offsets |
| `transport/ws/v1/sessionEventJournal.ts` | `src/transport/ws/v1/session_event_journal.rs` | Complete |
| `transport/ws/v1/subagentRosterTracker.ts` | `src/transport/ws/v1/subagent_roster_tracker.rs` | Complete |
| `transport/ws/v1/protocol.ts` | `src/transport/ws/v1/protocol.rs` | Complete |
| `routes/action-suffix.ts` | `src/routes/action_suffix.rs` | Complete |
| `lib/fileLaunch.ts` | `src/launch.rs` | Complete |
| `request-id.ts`, `requestLogging.ts`, `version.ts` | matching top-level Rust modules | Pure behavior complete |
| `services/pinoLoggerService.ts` | `src/services/server_logger.rs` | JSON logger counterpart complete |
| protocol DTO modules | sibling `kimi-code-protocol` crate | Reused rather than duplicated |
| `start.ts`, `listenWithPortRetry()` | `src/start.rs` | Axum listener, exposure policy, auth, instance registration, port retry and graceful transport shutdown complete |
| `routes/registerApiV1Routes.ts`, route modules | matching `src/routes/*.rs` files | Full documented interface surface with one named async handler per endpoint; core-dependent method bodies use the explicit bridge below |
| `routes/webAssets.ts` | `src/routes/web_assets.rs` | Async static files, MIME mapping, SPA fallback and reserved paths complete |
| `transport/ws/v1/registerWsV1.ts`, connection control methods | `src/web/websocket.rs` | Upgrade/auth/subprotocol, hello, control ACKs, registry and close lifecycle complete |

## Explicit agent-core-v2 Boundaries

These production paths intentionally use `todo!` with `MIGRATION-TODO`
comments, as requested, because their original implementation calls unfinished
`agent-core-v2` services:

- standalone instance-registry helpers called without a resolved home;
- full `SnapshotReader.read` assembly;
- RPC channel discovery/reflection;
- every REST handler represented by `CoreOperation`, through
  `TodoAgentCoreBridge::invoke`;
- WebSocket durable event replay/subscription validation and filesystem-watch
  operations, through the same bridge.

`RunningServer.close` completes transport shutdown, limiter disposal, WebSocket
close-all and instance-release ordering. Agent-core-v2 Scope disposal, the
model refresh scheduler, the durable event broadcaster and filesystem-watch
lifecycles remain documented integration points until those Rust services
exist.

## Remaining Integration Work

The Tokio/Axum HTTP and WebSocket adapter is operational. The next migration
units are the agent-core-v2-backed implementations behind `AgentCoreBridge`,
followed by durable WebSocket broadcast/replay, filesystem watches, endpoint
DTO validation parity and cross-implementation end-to-end fixtures.
