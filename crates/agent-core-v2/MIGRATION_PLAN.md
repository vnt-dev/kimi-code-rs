# agent-core-v2 migration plan

Original package: `packages/agent-core-v2`

## Baseline audit

- Original source inventory: 616 TypeScript declaration/implementation files under `src/`.
- Existing Rust inventory at the start of this migration: 65 files, covering selected `_base`, `kosong/contract`, protocol traits, and provider wire adapters.
- The existing crate compiles, but compilation covers only that subset and is not evidence that the package migration is complete.
- Target layout follows the original top-level domains: `_base`, `kosong`, `tool`, `wire`, `os`, `persistence`, `app`, `agent`, and `session`.

## External workspace dependencies

The following packages are already migrated and are consumed as Rust crates rather than reimplemented here:

- `@moonshot-ai/kimi-code-oauth` -> `kimi-code-oauth`
- `@moonshot-ai/minidb` -> `kimi-code-minidb`
- `@moonshot-ai/protocol` -> `kimi-code-protocol`

## Dependency layers

The source import graph has cycles between the business domains because runtime construction is mediated by the DI/scope container. Migration therefore separates service contracts from implementations and proceeds through these dependency layers:

1. `_base` leaf utilities, error values, events, lifecycle, logging, execution-environment helpers, and DI/scope infrastructure.
2. `kosong/contract` plus pure `tool` and `wire` data/validation modules.
3. `os/interface` and `persistence/interface` contracts.
4. `kosong/protocol`, provider adapters, model catalog/auth/discovery, then `os/backends` and `persistence/backends`.
5. App-scope foundations: bootstrap, config, event bus, flags, telemetry, file/edit/git, and catalogs.
6. Agent-scope contracts and pure operations: context, permissions, plan, goal, usage, tools, profiles, and task state.
7. Agent service implementations: request loop, tool execution, compaction, MCP/media, hooks, skills, swarm, and RPC.
8. Session-scope services: context, persistence views, filesystem/process, agent lifecycle, approval/interaction, subagents, swarm, cron, and todo.
9. Remaining app orchestration: auth, plugins, session lifecycle/export/index, workspace registry, gateway/web, root hooks/errors/exports.

Within each layer, pure data and deterministic functions are migrated before stateful or I/O services. I/O remains async-first with Tokio. Rust ownership may replace source DI patterns where necessary, while preserving service scope, call direction, state transitions, and observable behavior.

## Verification

- Maintain a source-to-Rust module inventory until every required source module has a counterpart or an explicitly documented Rust consolidation.
- Add only parity tests needed for meaningful behavior, boundaries, serialization, state transitions, and error paths.
- Before each migration commit: format, compile, run targeted tests, run strict Clippy, and inspect the staged diff.
- Completion requires a full inventory audit, no migration placeholders, and passing crate-level formatting, Clippy, and tests.
