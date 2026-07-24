# `agent-core-v2` migration plan

Original package: `/mnt/d/kimi/kimi-code/packages/agent-core-v2`
Rust target: `crates/agent-core-v2`

## Baseline audit (2026-07-24)

- The original package contains 616 TypeScript implementation/declaration files
  under `src/`.  Its own domain-layer checker passes for all 904 production and
  test files (`node scripts/check-domain-layers.mjs`).
- The Rust crate contains 397 source files, 303 of which name their original
  source in a mapping comment. `cargo check -p kimi-code-agent-core-v2` passes.
  This is a useful baseline only; it does **not** prove that the public surface
  or every original method has been migrated.
- The Rust crate has 301 source files containing unit tests. Tests remain
  targeted parity evidence rather than a file-count substitute for migration.
- The worktree was clean before this analysis. Existing committed Rust code is
  retained and audited against the source rather than replaced by a redesign.

### Source/Rust inventory by top-level domain

| Domain | TypeScript files | Rust files | Audit conclusion |
| --- | ---: | ---: | --- |
| `_base` | 47 | 49 | Comparable or consolidated; verify public exports while using it. |
| `kosong` | 61 | 57 | Core contracts/adapters exist; catalog, discovery, requester, and vendor parity still need an explicit audit. |
| `tool` | 7 | 7 | Comparable; retain as the L3 contract substrate. |
| `wire` | 13 | 8 | Deliberate consolidation is plausible; audit record/migration/public exports before declaring complete. |
| `os` | 20 | 19 | Interfaces and Node-local replacements are present; audit each host operation and tool-facing behavior. |
| `persistence` | 13 | 18 | Interfaces/backends are present; preserve file format and failure semantics in audit. |
| `app` | 148 | 109 | Missing top-level domains include cron, edit, gateway, session export/lifecycle/legacy, web, and legacy messages/auth. |
| `agent` | 223 | 110 | Largest gap: activity/context projection, media, MCP, policy/executor/select, plugin/profile/prompt, retry, RPC, and related tools. |
| `session` | 80 | 18 | Largest structural gap: lifecycle, cron, interaction/question, metadata/log, subagent/swarm/todo, terminal, and workspace command. |

The file totals are triage data, not a one-to-one completion criterion: a Rust
module may legitimately consolidate closely coupled source files, but each
source method still needs an identifiable counterpart or an explicit mapping.

## Dependency graph and migration boundaries

The original package's checked graph is the authority for migration order. The
source checker defines the following bottom-up dependency layers:

```text
L0  _base, errors, llm protocol, kosong/contract
L1  host/persistence interfaces, logging, event/bootstrap/environment,
    session/scope context, task primitive, kosong/protocol
L2  wire/blob/config/file/session FS/process/registries/auth/provider/model,
    persistence backends
L3  tool and policy/catalog/registry capabilities
L4  agent behaviour: context, injection/compaction, plan/goal/usage, loop,
    media, prompting, tool selection/execution, MCP support
L5  agent task, MCP and cron orchestration
L6  agent/session coordination: lifecycle, subagents, hooks, exports,
    interaction, terminal, workspace commands
L7  user-facing boundary: approval, questions, gateway/RPC and legacy facades
```

The high-volume direct import edges reinforce that order: provider depends on
the `kosong` contract/protocol layers; model depends on provider and config;
host backends depend on the host interfaces and tool contract; persistence
backends depend on persistence interfaces; and session catalog domains consume
the app catalogs. The only source graph cycles are composition/scope cycles;
they must be broken in Rust at service contracts and constructors, never by
changing observable call order.

External workspace dependencies are already Rust crates and must be consumed,
not reimplemented here:

- `@moonshot-ai/kimi-code-oauth` -> `kimi-code-oauth`
- `@moonshot-ai/minidb` -> `kimi-code-minidb`
- `@moonshot-ai/protocol` -> `kimi-code-protocol`

For source-only dependencies (for example MCP SDK, image decoding, HTML
readability, glob ignore matching, and vendor LLM SDKs), select a maintained
Rust crate only when the corresponding migration unit is reached. The chosen
crate must preserve the source package's protocol, error, and ordering
semantics; otherwise implement the needed adapter locally.

## Execution units, in order

Every row is a separate commit unless two entries share a type/state invariant
that cannot be compiled or tested separately. For each unit, first add source
mapping comments identifying original files and primary methods.

1. **Close the L0--L2 audit gaps.** Audit existing `_base`, `wire`, `tool`,
   `os`, and `persistence` modules against their original exports. Port any
   missing leaf methods before higher-level callers. In particular, complete
   the missing `kosong` model catalog/discovery/requester and vendor-definition
   methods, with request/response serialization tests.
2. **Complete app L1--L3 foundations.** Migrate the absent app domains in
   dependency order: `edit`, `cron`, `sessionIndex`, `sessionLifecycle`,
   `sessionExport`, `sessionLegacy`, `gateway`, `web`, and legacy facades.
   Preserve config keys, persisted records, archive layout, and event order.
3. **Complete agent L3 contracts first.** Port `permissionGate`,
   `toolExecutor`, `toolSelect`, `toolDedupe`, `userTool`, and the missing
   policy rule methods, then their registration/side-effect modules. Add only
   rule, conflict, state-transition, and error-path parity tests.
4. **Complete agent L4 state and execution.** Port context projection,
   profile/prompt/injection, activity view, step retry, replay builder, media,
   LLM requester, loop, compaction, skill/plugin, and MCP methods. Keep pure
   transforms synchronous; make filesystem, process, network, timer, and
   stream methods async with Tokio while preserving sequential source order.
5. **Complete agent coordination.** Port task/goal/swarm helper tools and
   RPC-facing agent behavior that depend on the completed state/execution
   layer. Test replay/restore and cancellation boundaries.
6. **Complete session L1--L6 services.** Port session log/metadata, terminal,
   interaction/questions, lifecycle, agent lifecycle, subagent, cron, swarm,
   todo, workspace command/init, and session-scoped catalogs/policies. Retain
   scoped lifetime, shutdown, approval, and persistence semantics.
7. **Public-surface and completion audit.** Map every source file and public
   export to Rust or a documented consolidation; remove migration TODOs; test
   serialized wire/config/archive compatibility; run full crate/workspace
   validation; and only then treat the package as migrated.

## Required evidence for each commit

- Original file/method -> Rust file/method mapping, including sync/async choice
  and Rust-specific ownership or error adaptation.
- Focused parity tests for externally observable behavior, boundaries, state
  transitions, serialization, and errors relevant to that unit.
- `cargo fmt --all --check`
- `cargo check -p kimi-code-agent-core-v2`
- Targeted `cargo test -p kimi-code-agent-core-v2 <module>`
- `cargo clippy -p kimi-code-agent-core-v2 --all-targets -- -D warnings`
- Explicit staging and an atomic commit whose message names the migrated unit.

Final completion additionally requires `cargo check --workspace`, `cargo test
--workspace`, and strict workspace Clippy, plus an inventory audit proving that
no original `agent-core-v2` logic remains unported or silently stubbed.
