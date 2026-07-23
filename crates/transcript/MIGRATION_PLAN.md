# transcript migration plan

## Scope

- Original package: `D:\kimi\kimi-code\packages\transcript`
- Original baseline: `e45832398d0d9cad98dbad1cbf1e5b103a20aace`
- Rust target: `D:\kimi\kimi-code-rs\crates\transcript`
- Rust crate: `kimi-code-transcript`
- Included: all production logic exported from the original `src/index.ts`.
- Included tests: only tests needed to prove wire compatibility, state transitions,
  ordering, pagination, history reconstruction, and important error paths.
- Excluded: `packages/kap-server`, `apps/kimi-inspect`, the root application, and
  every other Rust crate.
- The only allowed files outside `crates/transcript` are the root `Cargo.toml`
  workspace-member entry and the generated `Cargo.lock` package entry.

The source path given in the initial request included
`apps/kimi-code/packages/transcript`; the package present in the source checkout is
`D:\kimi\kimi-code\packages\transcript`.

## Baseline inventory

- 21 TypeScript source files, approximately 2,704 lines.
- 2 Vitest files containing 43 behavior cases.
- One third-party runtime dependency: `zod`.
- No dependency on `@moonshot-ai/protocol`, `agent-core-v2`, or another source
  workspace package.
- Known consumers, inspected only to identify the public contract:
  - `packages/kap-server`
  - `apps/kimi-inspect`
- The original package is synchronous and browser-safe. It performs no filesystem,
  network, process, timer, or other blocking I/O.

The original TypeScript test command could not be established as an executable
baseline in the current checkout: `node_modules` contains `.pnpm` but no `.bin`,
and `pnpm --filter @moonshot-ai/transcript typecheck` produced no output before
timing out. The checked-in tests and source behavior are therefore the parity
specification. This limitation must remain visible in the completion report.

## Source dependency graph

The source import graph is acyclic.

```text
model/ids
├── model/attachment
├── model/interaction
├── model/task
├── model/todo
├── model/frame ──────────────┐
│   └── model/interaction     │
├── model/turn                │
│   └── model/frame           │
└── model/item                │
    └── model/turn            │
                              ▼
all model modules ──────> ops/operation
                              │
                              ▼
                         ops/apply
                              │
                              ▼
                    store/agentTranscript
                              │
                              ▼
                   store/transcriptStore

granularity/grade ──────> granularity/filterOps <──── ops/operation
model/ids + model/item ─> pagination/paginate
frame + task + turn ────> view/registry
models + operation ─────> history/groupTurns
all wire-facing types ──> wire/schema ──────────────> wire/events
all modules ────────────> index.ts / Rust lib.rs
```

There are no grounds for creating additional Rust crates. The original module
domains will remain modules within `crates/transcript`.

## Planned Rust module mapping

```text
Original                                      Rust
src/model/ids.ts                              src/model/ids.rs
src/model/attachment.ts                       src/model/attachment.rs
src/model/frame.ts                            src/model/frame.rs
src/model/interaction.ts                      src/model/interaction.rs
src/model/item.ts                             src/model/item.rs
src/model/meta.ts                             src/model/meta.rs
src/model/task.ts                             src/model/task.rs
src/model/todo.ts                             src/model/todo.rs
src/model/turn.ts                             src/model/turn.rs
src/ops/operation.ts                          src/ops/operation.rs
src/ops/apply.ts                              src/ops/apply.rs
src/store/agentTranscript.ts                  src/store/agent_transcript.rs
src/store/transcriptStore.ts                  src/store/transcript_store.rs
src/granularity/grade.ts                      src/granularity/grade.rs
src/granularity/filterOps.ts                  src/granularity/filter_ops.rs
src/pagination/paginate.ts                    src/pagination/paginate.rs
src/view/registry.ts                          src/view/registry.rs
src/history/groupTurns.ts                     src/history/group_turns.rs
src/wire/schema.ts                            src/wire/schema.rs
src/wire/events.ts                            src/wire/events.rs
src/index.ts                                  src/lib.rs
```

Names exposed only in Rust follow Rust conventions (`snake_case` methods and
modules, `UpperCamelCase` types). Serde renames preserve all external camelCase,
snake_case, dotted operation names, and dotted event names.

## Dependency and type strategy

### Libraries

- `serde`: typed serialization and deserialization.
- `serde_json`: TypeScript `unknown` payloads and open content envelopes.
- `indexmap`: JavaScript `Map`/`Set` insertion-order compatibility.
- Handwritten validation replaces `zod`; no general validation framework is
  needed for this small, closed schema.

No async runtime is needed. All migrated functions remain synchronous because
they are in-memory parsing, reduction, indexing, grouping, and dispatch logic.

### Identifiers

TypeScript aliases all identifiers to `string`. Rust uses transparent newtypes
(`TurnId`, `StepId`, `FrameId`, and so on) to prevent accidental identifier
mixing while keeping the serialized representation exactly a JSON string.

### Open JSON fields

The following fields accept opaque JSON:

- origin and marker payloads
- tool input/output/display
- interaction request/response
- notice detail

The representation must distinguish:

```text
property absent       TypeScript optional field was omitted
property: null        explicit JSON null
property: <value>     actual JSON value
```

Collapsing absent and `null` into a single `Option<Value>` would change wire
round-tripping, so the Rust model uses a double-option Serde adapter.

### Numeric behavior

- Wire-validated ordinals, offsets, and page sizes use fixed-width integers,
  never `usize`.
- Token, cost, size, and budget fields remain floating-point JSON numbers where
  the original schema used unrestricted `z.number()`.
- Turn-id helpers preserve the original invalid-id fallback to ordinal zero.
- Reducer arithmetic must not depend on debug/release overflow differences.

### Ordered state

The following observable JavaScript insertion orders must be retained:

- task, interaction, attachment, and todo maps when materializing snapshots
- the pending-interaction set
- the session agent roster
- view renderer registrations where iteration becomes observable

`IndexMap` and `IndexSet` will be used rather than `HashMap`/`HashSet`.

### Copy-on-write and equality

Previously returned snapshots must never be mutated by later applies. Rust will
clone only the affected branches and retain immutable prior values.

The TypeScript reducer sometimes uses JavaScript reference equality for nested
objects and arrays even though the documented operation contract describes
state upserts as idempotent. This is a source quirk, not permission to redesign
the reducer. Reducer tests will cover accepted-op and notification behavior for
replayed operations. Any Rust ownership adjustment that cannot reproduce a
reference-identity detail exactly must be documented explicitly rather than
silently treated as a cleanup.

### UTF-16 append offsets

`appendAtOffset` uses JavaScript string `.length` and `.slice`, whose offsets are
UTF-16 code units. Rust strings are UTF-8. A direct `String::len()` port would
misplace chunks after non-ASCII characters and could panic at non-boundary byte
indices.

The Rust `append_at_offset` implementation will:

1. measure offsets in UTF-16 code units;
2. compare overlap in UTF-16;
3. preserve duplicate, partial-overlap, mismatch-gap, and beyond-tail-gap
   behavior;
4. convert back to a valid Rust `String` without accepting invalid surrogate
   output.

Parity tests will include BMP and surrogate-pair characters.

## Migration units and commits

Each unit is formatted, compiled, tested, linted, reviewed, explicitly staged,
and committed before the next unit begins.

### Unit 1: model and crate foundation

Status: completed in commit `17c77c7` (`migrate: port transcript model types to Rust`).

Original:

- `src/model/*.ts`
- the model exports in `src/index.ts`

Rust:

- crate manifest and workspace registration
- `src/model/*.rs`
- optional/null Serde support
- initial `src/lib.rs`

Validation completed:

- `cargo fmt --manifest-path crates/transcript/Cargo.toml -- --check`
- `cargo check -p kimi-code-transcript`
- `cargo test -p kimi-code-transcript`
- `cargo clippy -p kimi-code-transcript --all-targets -- -D warnings`

### Unit 2: operation vocabulary

Status: completed in commit `af56c70`
(`migrate: port transcript operation vocabulary`).

Original:

- `src/ops/operation.ts`

Rust:

- `src/ops/mod.rs`
- `src/ops/operation.rs`

Behavior and formats:

- all 13 operation variants
- dotted `op` discriminants
- turn/step headers without child collections
- reset snapshot and defaultable global collections
- append target variants
- accepted operations and gap reporting

Necessary tests:

- representative serialization plus deserialization of every operation kind
- explicit checks for dotted discriminants and camelCase fields

### Unit 3: pure reducer

Status: completed in commit `618c58d`
(`migrate: port transcript operation reducer`), with the total frame-append
branch cleanup in `debceb1`.

Original:

- `applyOperation()`
- `appendAtOffset()`
- private reducer helpers in `src/ops/apply.ts`

Rust:

- `src/ops/apply.rs`

Behavior:

- reset replacement and pending-interaction derivation from both channels
- turn/step/frame auto-vivification
- ordinal ordering and anchored standalone-item insertion
- state upserts and no-op detection
- append duplicate/overlap/gap behavior
- task append auto-vivification
- meta shallow merge and explicit mode clearing
- item removal plus anchored interaction cleanup
- copy-on-write prior-snapshot stability

Necessary tests:

- one end-to-end turn/step/frame reduction case
- missing-parent auto-vivification and operation replay
- UTF-16 append duplicate/overlap/gap table
- pending interaction tracking and removal cleanup
- meta mode set/keep/clear
- anchored item ordering and snapshot immutability

### Unit 4: agent and session stores

Status: completed in commit `82c460c` (`migrate: port transcript stores`).

Original:

- `AgentTranscript`
- `TranscriptStore`

Rust:

- `src/store/agent_transcript.rs`
- `src/store/transcript_store.rs`

Behavior:

- one convergence path for `receive` and `apply`
- accepted-operation collection
- last gap returned while later operations continue
- exactly one change notification per mutating batch
- listener disposal
- read accessors
- tail-turn snapshot windowing and `hasMoreOlder`
- lazy agent creation, roster notifications, removal, description, disposal

The Rust listener ownership mechanism may differ structurally, but callback
order, batching, removal, and observable state must remain the same.

Necessary tests:

- accepted batch and gap behavior
- one-notification-per-batch plus disposal
- snapshot tail window segmentation
- lazy roster and disposal state

### Unit 5: granularity, pagination, and view registry

Status: completed in commit `1a12f37`
(`migrate: port transcript presentation layers`).

Original:

- `src/granularity/grade.ts`
- `src/granularity/filterOps.ts`
- `src/pagination/paginate.ts`
- `src/view/registry.ts`

Rust:

- matching `granularity`, `pagination`, and `view` modules

Behavior:

- grade ranking, wildcard fallback, and upgrade reset detection
- operation filtering and append-only detection
- snapshot redaction below block grade
- turn-segment pagination in both directions
- marker-only and leading-head-unit edge cases
- case-insensitive tool dispatch and fallback renderer
- input and marker renderer lookup

Necessary tests:

- one table-driven granularity test
- pagination boundary table
- one registry dispatch test

### Unit 6: cold history reconstruction

Status: completed in commit `eaffed3`
(`migrate: port transcript history reconstruction`).

Original:

- `groupMessagesIntoSnapshot()`
- all helpers in `src/history/groupTurns.ts`

Rust:

- `src/history/group_turns.rs`

Behavior:

- skip system messages
- hide injection/system-trigger/retry content
- open promptless turns for goal continuation and subagent triggers
- create markers for skills, plugin commands, and compaction
- user-slash skill/plugin activations also open turns
- assign zero-based turn ordinals
- convert one assistant message into one step
- parse tool arguments as JSON with raw-string fallback
- fold persisted tool results into prior tool frames
- map cron/task/hook/shell origins
- extract attachment metadata while dropping base64 bytes

Necessary tests:

- ordinary messages plus tool result folding
- media conversion and base64 omission
- hidden origin versus turn-opening trigger
- user-slash versus mid-turn skill activation
- cron and legacy background-task origin mapping

### Unit 7: wire validation and events

Status: completed in commit `7311162`
(`migrate: port transcript wire validation`).

Original:

- `src/wire/schema.ts`
- `src/wire/events.ts`

Rust:

- `src/wire/schema.rs`
- `src/wire/events.rs`

Behavior:

- closed discriminated model and operation structures
- unknown open-content fields
- non-empty ids where the Zod schema requires them
- integer and non-negative constraints
- snapshot/response defaults for interactions, attachments, and todos
- grade and subscription maps
- mutually exclusive transcript cursors
- page-size range `1..=100`
- path-safe agent ids: ASCII `[A-Za-z0-9._-]`, length `1..=128`,
  excluding `.` and `..`
- REST snake_case fields
- `transcript.reset` and `transcript.ops` event discriminants

Necessary tests:

- all operation variants parse
- missing backward-compatible global collections default to empty
- bad grades and mutually exclusive cursors fail
- complete hostile-agent-id table
- event round-trip

### Unit 8: completion audit

Status: completed. Audit correction commit `b449b1e`
(`fix: preserve transcript step wire discriminants`).

Actions:

1. Compare every original source file and public export with the Rust tree.
2. Search for `TODO`, `todo!`, `unimplemented!`, placeholder returns, ignored
   errors, and unexplained panics.
3. Confirm no source method or operation variant is missing.
4. Confirm external field names and defaults from `wire/schema.ts`.
5. Review all commits and ensure no unrelated files were included.
6. Run:

```text
cargo fmt --manifest-path crates/transcript/Cargo.toml -- --check
cargo check -p kimi-code-transcript
cargo test -p kimi-code-transcript
cargo clippy -p kimi-code-transcript --all-targets -- -D warnings
```

7. Run workspace-level checks only after confirming they will not format or
   overwrite unrelated user work. Failures outside this crate are reported
   separately and are not modified under this task.

Any audit correction is committed as a focused transcript-only commit.

## Completion audit results

- All 21 original TypeScript source files have the Rust counterpart listed in
  the module mapping above.
- All 13 operation variants and both transcript event variants are represented
  by closed Rust enums with their original dotted wire discriminants.
- The original per-variant operation interfaces are consolidated into
  `TranscriptOperation` variants. The named reset/ops event interfaces remain
  identifiable as `TranscriptResetEvent` and `TranscriptOpsEvent`.
- The original Zod schema constants are consolidated into the corresponding
  Serde types plus `WireValidate`; `parse_wire_value` and `parse_wire_json` are
  the common validated parse paths.
- Literal `kind` fields that are redundant in Rust's in-memory enum variants
  are restored by wire adapters. The audit found and corrected a missing
  nested step discriminator; the complete response round-trip test covers
  turn, step, frame, marker, taskref, task, interaction, attachment, todo,
  metadata, roster, snake_case REST fields, camelCase model fields, open JSON,
  and explicit null.
- `TranscriptListener`, `RosterListener`, `TranscriptResetEvent`, and
  `TranscriptOpsEvent` remain named public counterparts for source
  traceability.
- Searches found no production `TODO`, `MIGRATION-TODO`, `todo!`,
  `unimplemented!`, or unexplained `unreachable!`.
- Commits made after the migration branch point modify only
  `crates/transcript`; the earlier crate-foundation commit additionally
  contains only the allowed root workspace and lockfile entries.

Final crate validation:

```text
cargo fmt --manifest-path crates/transcript/Cargo.toml -- --check   passed
cargo check -p kimi-code-transcript                                passed
cargo test -p kimi-code-transcript                                 passed (30 tests)
cargo clippy -p kimi-code-transcript --all-targets -- -D warnings  passed
```

Workspace validation:

```text
cargo fmt --all -- --check                                         passed
cargo check --workspace                                            passed
cargo clippy --workspace --all-targets --all-features -- -D warnings
                                                                    passed
cargo test --workspace                                             failed outside transcript:
  348 passed, 2 failed in kimi-code-agent-core-v2
```

The two workspace failures are pre-existing and outside this task's allowed
scope: a Windows path-separator assertion in `log_config` and an OpenAI
capability pointer-identity assertion. No external crate was changed.

## Rust-specific adaptations and known limitations

- The package remains fully synchronous because every operation is in-memory;
  no async runtime is needed.
- Copy-on-write aggregate branches use `Arc`; JavaScript `Map`/`Set` insertion
  order uses `IndexMap`/`IndexSet`.
- JavaScript reference identity for nested reducer payloads and roster
  descriptors has no direct value-type Rust equivalent. Rust uses value
  equality for those fields. The source's observable modes-object recreation
  behavior is preserved explicitly.
- Listener storage uses shared single-threaded callback registries. Explicit
  `dispose()` controls removal, and dropping a disposal handle alone does not
  unregister the callback, matching the source lifecycle.
- Append offsets use UTF-16 code units. Rust cannot represent an unpaired
  surrogate string, so an append that would create one is reported as a gap
  rather than creating an invalid Rust `String`.
- The original TypeScript test runner remained unavailable in the source
  checkout; the checked-in TypeScript source/tests and the pinned source
  commit are the parity specification.

## Definition of done

The migration is complete only when:

- every one of the 21 original source files has an identifiable Rust
  counterpart or documented module consolidation;
- every public runtime function, class method, type, constant, operation, and
  event has a Rust counterpart;
- reducer ordering, state transitions, gap handling, cleanup, and notifications
  match the source;
- JSON field names, discriminants, defaults, optional/null behavior, and query
  validation match the source;
- the necessary parity tests pass;
- format, check, test, and strict Clippy pass for `kimi-code-transcript`;
- no placeholders or unexplained incomplete branches remain;
- commits contain no unrelated module changes;
- the completion report lists files, sync/async decision, structural Rust
  adaptations, commands and results, commit hashes, limitations, and confirms
  that migration stops after `transcript`.
