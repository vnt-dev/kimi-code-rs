# AGENTS.md

## Objective

Migrate the existing project to Rust while preserving its externally observable behavior.

The migration should be performed primarily at the **method level**. Each significant original method should have a clearly identifiable Rust counterpart.

The Rust implementation must:

* Preserve business logic and observable behavior.
* Follow Rust naming and coding conventions.
* Follow standard Cargo project conventions.
* Prefer asynchronous APIs for blocking or waiting operations.
* Remain easy to compare with the original implementation.
* Be implemented and committed in small, verifiable units.

This is a migration project, not a full redesign.

## Priority Order

When requirements conflict, use the following priority:

1. Preserve externally observable behavior.
2. Preserve protocols, serialized data, and persistent formats.
3. Preserve state transitions, errors, ordering, and side effects.
4. Preserve method-level traceability.
5. Satisfy Rust memory-safety requirements.
6. Follow Rust project and naming conventions.
7. Use idiomatic Rust.
8. Prefer asynchronous I/O.
9. Optimize only after equivalence is established.

## Method-Level Migration

Prefer translating one original method or one tightly coupled method group at a time.

For each migrated method, identify:

* The original file and method.
* The corresponding Rust file and method.
* Inputs and outputs.
* State changes and side effects.
* Error conditions.
* External call order.
* Whether the Rust method is synchronous or asynchronous.
* Any structural changes required by Rust.

Prefer a clear mapping:

```text
Original                         Rust
parsePacket()                    parse_packet()
loadConfiguration()             load_configuration().await
updateSessionState()            update_session_state()
sendResponse()                  send_response().await
```

Do not unnecessarily:

* Merge unrelated original methods.
* Split a simple method into many tiny helpers.
* Move logic across unrelated modules.
* Hide control flow behind complex abstractions.
* Replace understandable code with excessive macros or generics.

A method may be split when required for ownership, async boundaries, error conversion, resource management, platform abstraction, or testing.

When splitting a method, keep one primary Rust method that clearly represents the original method.

## Behavioral Compatibility

Preserve:

* Input-to-output behavior.
* Branch conditions.
* State transitions.
* Error conditions.
* Side effects.
* Ordering requirements.
* Timeout and retry behavior.
* Resource cleanup.
* Boundary-case behavior.

Code may be reorganized when required by Rust, provided that observable behavior does not change.

Do not silently fix suspected bugs in the original implementation. Preserve the behavior, document it, and handle any fix separately.

Successful compilation alone does not prove equivalence.

## Rust Naming and Style

Follow standard Rust naming conventions:

* `snake_case` for functions, methods, variables, modules, and files.
* `UpperCamelCase` for structs, enums, traits, and enum variants.
* `SCREAMING_SNAKE_CASE` for constants and statics.
* Conventional names such as `new`, `default`, `from_*`, `try_*`, `as_*`, and `into_*`.

Use Rust-style internal names even when external formats use another convention.

```rust
#[derive(serde::Serialize, serde::Deserialize)]
pub struct UserRequest {
    #[serde(rename = "userId")]
    pub user_id: String,
}
```

Prefer:

* `Option<T>` for optional values.
* `Result<T, E>` for recoverable errors.
* Typed error enums.
* Pattern matching.
* RAII for resource management.
* Borrowing instead of unnecessary cloning.
* Composition instead of inheritance.
* Traits only for meaningful abstraction.
* Narrow visibility such as `pub(crate)` or private items.
* `From`, `TryFrom`, `AsRef`, and other standard conversion traits where appropriate.

Avoid:

* Mechanical translation of object-oriented patterns.
* Java-style utility classes.
* C-style global mutable state.
* Unnecessary getters and setters.
* Excessive traits, generics, macros, or dynamic dispatch.
* Unnecessary `Arc<Mutex<T>>`.
* Excessive `clone()`.
* Production use of `unwrap()` or `expect()` without justification.
* Boolean parameters whose meaning is unclear.
* String-based states that should be enums.

Rust-style adjustments are allowed when they preserve behavior and keep the original method mapping understandable.

## Rust Project Structure

Follow normal Cargo conventions.

Use a Cargo workspace only when there are meaningful crate boundaries, such as separate binaries, reusable libraries, platform implementations, or clear dependency layers.

Do not create extra crates only to reproduce the original directory tree.

Keep public APIs intentional and visibility as narrow as practical.

## Type and Data Mapping

Choose Rust types according to semantics, not only source-language names.

Typical mappings:

```text
Nullable value                  Option<T>
Recoverable operation           Result<T, E>
Dynamic array                   Vec<T>
Byte buffer                     Vec<u8>, Bytes, BytesMut
Map                             HashMap<K, V>, BTreeMap<K, V>
Set                             HashSet<T>, BTreeSet<T>
Shared immutable ownership      Arc<T>
Single-thread shared ownership  Rc<T>
Shared mutable state            Arc<Mutex<T>>, Arc<RwLock<T>>
Finite state                    enum
Distinct identifier             Newtype struct
Bit flags                       bitflags
```

Select integer types according to:

* Bit width.
* Signedness.
* Protocol or file representation.
* Database representation.
* Overflow behavior.
* Platform dependence.

Do not use `usize` for protocol, serialized, or persisted values unless they are explicitly platform-sized.

Preserve external field names, enum representations, byte order, time units, defaults, null behavior, numeric precision, and unknown-field behavior.

## Function Signatures

Use idiomatic Rust signatures while preserving method semantics.

Prefer:

```rust
pub fn parse(data: &[u8]) -> Result<Packet, ParseError>
```

instead of:

```rust
pub fn parse(data: &Vec<u8>) -> Result<Packet, ParseError>
```

Use:

* `T` when the method consumes a value.
* `&T` for read-only access.
* `&mut T` for exclusive mutation.
* `Arc<T>` only when shared ownership is required.
* `Cow<'_, T>` only when both borrowed and owned forms are useful.

Do not create complicated lifetime designs only to avoid a harmless clone, but do not use cloning as the default solution.

## Async-First Design

Prefer async Rust for operations that may block or wait:

* Network I/O.
* Database operations.
* IPC.
* Timers.
* Message queues.
* External services.
* Process waiting.
* Concurrent request handling.
* Long-running server operations.
* File I/O when async I/O is useful.

Use the runtime already selected by the project. When no runtime exists and general-purpose async I/O is needed, prefer Tokio unless project constraints require something else.

Pure computation should normally remain synchronous:

* Parsing an in-memory buffer.
* Checksums.
* Encoding into memory.
* Local validation.
* Local state updates.
* Short CPU-only transformations.

Example:

```rust
pub async fn receive_packet(
    &mut self,
) -> Result<Packet, ReceiveError> {
    let bytes = self.read_packet_bytes().await?;
    parse_packet(&bytes).map_err(ReceiveError::Parse)
}

pub fn parse_packet(data: &[u8]) -> Result<Packet, ParseError> {
    // Synchronous in-memory parsing.
}
```

Async does not automatically mean concurrent.

Do not replace sequential operations with `join!`, `try_join!`, or spawned tasks unless the operations are proven independent and concurrent execution preserves behavior.

## Blocking Work in Async Code

Do not run long blocking operations directly on async runtime worker threads.

Prefer async libraries such as:

```text
std::net                 tokio::net
std::process             tokio::process
std::thread::sleep       tokio::time::sleep
blocking database API    async database API
```

When blocking work cannot be replaced, use an explicit blocking boundary:

```rust
let result = tokio::task::spawn_blocking(move || {
    perform_blocking_operation()
})
.await
.map_err(TaskError::Join)??;
```

Do not use `spawn_blocking` for small and fast synchronous calculations.

Use `std::sync::Mutex` for short critical sections that never cross `.await`.

Use `tokio::sync::Mutex` only when a lock must remain held across `.await`. Prefer designs that avoid holding locks across `.await`.

## Concurrency and Task Lifecycle

Introduce concurrency only when:

* The original implementation is concurrent.
* Operations are independent.
* Concurrency is required for responsiveness or performance.
* Behavior has been verified under concurrency.

Preserve:

* Ordering.
* Locking semantics.
* Maximum concurrency.
* Backpressure.
* Cancellation.
* Timeout behavior.
* Shutdown behavior.
* Background-task lifecycle.

Do not create unmanaged fire-and-forget tasks.

Background tasks must have:

* Error handling.
* Defined ownership.
* Cancellation or shutdown behavior.
* A clear lifetime.
* Cleanup when the application stops.

Use structured concurrency primitives such as `JoinSet`, channels, semaphores, `select!`, and cancellation tokens when appropriate.

## Cancellation, Timeout, and Retry

Async methods must not leave corrupted or partially updated state when cancelled.

Pay particular attention to methods used with:

* `tokio::select!`
* Timeouts.
* Task cancellation.
* Connection shutdown.

Use transactions, temporary buffers, state guards, or explicit commit steps when an operation must be atomic.

Preserve original timeout and retry semantics:

* Timeout duration.
* Per-attempt or total timeout.
* Maximum attempts.
* Retry delay.
* Backoff behavior.
* Retryable error types.
* State reset between attempts.
* Final error after exhaustion.

Do not improve or change retry behavior during migration unless explicitly requested.

## Error Handling

Use structured error types.

```rust
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("failed to read configuration")]
    Io(#[from] std::io::Error),

    #[error("invalid configuration: {0}")]
    InvalidConfiguration(String),

    #[error("operation timed out")]
    Timeout,
}
```

Preserve:

* Error conditions.
* Error categories.
* Error codes.
* Retryability.
* Error propagation.
* Externally visible messages when required.

Use typed errors in libraries and domain logic.

Use `anyhow` mainly at application boundaries where callers do not need to match individual error variants.

Avoid `unwrap()`, `expect()`, `panic!()`, `todo!()`, and `unimplemented!()` in production paths unless the original behavior is intentionally unrecoverable or the code is explicitly marked incomplete.

## Serialization and Protocols

External compatibility takes priority over internal style.

Preserve:

* Field names and types.
* Optional and null fields.
* Default values.
* Enum representations.
* Unknown-field behavior.
* Numeric precision.
* Time formats and units.
* Byte order.
* Length encoding.
* Message ordering.
* Error codes.
* Binary layout where externally relevant.

Use Serde attributes or adapters to keep external formats stable while using Rust naming internally.

For binary protocols, add byte-for-byte compatibility tests where practical.

## Numeric Behavior

Preserve:

* Signedness and bit width.
* Overflow and underflow behavior.
* Rounding and truncation.
* Floating-point precision.
* Shift behavior.
* Endianness.
* Division-by-zero behavior.
* NaN and infinity behavior.

Use explicit arithmetic when semantics matter:

```rust
wrapping_add
checked_add
saturating_add
overflowing_add
```

Do not depend on differences between debug and release overflow behavior.

## Resource Management

Use RAII for files, sockets, locks, transactions, buffers, temporary files, and foreign handles.

When explicit flushing, shutdown, commit, rollback, or cleanup order is required, implement it explicitly instead of relying only on `Drop`.

Preserve the original cleanup order when it is observable.

## Unsafe Code

Use safe Rust by default.

Use `unsafe` only when required for FFI, operating-system APIs, raw memory, memory-mapped hardware, or behavior that cannot reasonably be implemented safely.

Every `unsafe` block must:

* Be as small as practical.
* Include a `SAFETY` comment.
* State required invariants.
* Be wrapped by a safe API where practical.
* Have relevant tests.

Do not use `unsafe` only to bypass ownership or borrowing problems.

## Dependencies

Prefer stable, maintained, widely used crates.

Before adding a dependency, check:

* Whether the standard library is sufficient.
* Whether an existing project dependency already provides the feature.
* Target-platform support.
* Async-runtime compatibility.
* Maintenance status.
* Runtime and dependency-tree cost.

Do not replace core behavior merely because a crate offers a different implementation.

## Source Traceability

Add source mapping comments at significant module or method boundaries when useful.

```rust
// Original:
//   src/session/SessionManager.java
//   SessionManager.loadSession()
//
// Rust adaptation:
//   Converted blocking storage access to async.
//   Cache lookup order and not-found behavior are unchanged.
```

Use mapping comments for significant methods, state machines, protocol handling, complex control flow, and substantial Rust-specific adaptations.

Do not add source comments to every line.

## Temporary Implementations

Do not silently add placeholders, fake values, empty functions, ignored errors, or incomplete branches.

Mark incomplete work explicitly:

```rust
// MIGRATION-TODO:
// Original: src/network/Client.java, Client.connect()
// Missing dependency: TLS transport has not been migrated
// Temporary behavior: returns ConnectError::TlsUnavailable
// Completion condition: migrate the TLS transport
```

Temporary implementations must not be reported as complete.

## Testing

Tests are the primary evidence of equivalence.

For each migrated method or method group, test relevant cases:

* Normal input.
* Empty and invalid input.
* Boundary values.
* Error paths.
* State transitions.
* Repeated calls.
* Timeout and retry behavior.
* Cancellation.
* Concurrent behavior.
* Serialization.
* Binary compatibility.
* Resource cleanup.
* Known quirks of the original implementation.

When practical, run the same test vectors against both implementations and compare:

* Return values.
* Errors.
* State changes.
* Serialized output.
* Generated files.
* Network messages.
* Database changes.
* Other externally visible effects.

Use deterministic async tests. Avoid real sleeps and external networks when controlled time or mocks can be used.

## Migration Workflow

For each unit:

1. Read the original method, callers, dependencies, and tests.
2. Identify inputs, outputs, state changes, errors, ordering, and side effects.
3. Decide whether the Rust method should be synchronous or asynchronous.
4. Define an idiomatic Rust signature.
5. Implement the method while preserving behavior.
6. Add or migrate tests.
7. Compare behavior with the original implementation.
8. Run formatting, compilation, tests, and Clippy.
9. Review the diff.
10. Commit the completed unit.
11. Start the next independent unit only after committing the current unit.

Prefer method-level units.

Several methods may be included in one unit only when they are tightly coupled and cannot reasonably be compiled, tested, or reviewed separately.

## Implement One Unit, Commit One Unit

Complete one coherent and independently verifiable migration unit, test it, and commit it before starting the next independent unit.

Expected workflow:

```text
Implement unit A
Test unit A
Review unit A
Commit unit A

Implement unit B
Test unit B
Review unit B
Commit unit B
```

Do not accumulate many completed methods and create one large commit later.

A migration unit may be:

* One method.
* One tightly coupled method group.
* One struct and its core methods.
* One enum and its state transitions.
* One parser or encoder.
* One asynchronous I/O operation.
* One error type and its direct integration.
* One small module that cannot reasonably be separated.

Each commit should be understandable, testable, reviewable, revertible, and limited to one purpose.

## Commit Requirements

Before committing:

* Format the code.
* Ensure the relevant package compiles.
* Run relevant tests.
* Run relevant Clippy checks.
* Remove or document placeholders.
* Review staged files.
* Keep unrelated user changes untouched.
* Ensure the commit does not depend on unstaged changes.

Prefer explicit staging:

```bash
git add src/session.rs tests/session_tests.rs
git diff --cached
```

Avoid `git add .` unless every change has been reviewed and belongs to the current unit.

Use clear English commit messages:

```text
migrate: port packet parsing to Rust
migrate: port async socket connection to Rust
test: add parity tests for packet parsing
build: add Tokio runtime support
```

For nontrivial commits, describe:

* Original file and method.
* Rust file and method.
* Behavior migrated.
* Rust-specific adjustments.
* Async changes.
* Tests executed.
* Known limitations.

Do not use vague messages such as `update code`, `fix things`, `migration`, or `work in progress`.

## Git Safety

Do not:

* Rewrite shared history.
* Force-push.
* Rebase shared branches.
* Amend shared commits.
* Squash existing commits without instruction.
* Reset or discard user changes.
* Commit secrets, binaries, editor files, or unrelated formatting.

Do not run the following without explicit instruction:

```bash
git reset --hard
git clean -fd
git push --force
git push --force-with-lease
git rebase
git commit --amend
```

When unrelated changes exist, leave them untouched and stage only files belonging to the current migration unit.

## Validation Commands

Run relevant targeted checks before each unit commit:

```bash
cargo fmt --all --check
cargo check -p package_name
cargo test -p package_name method_or_module_name
cargo clippy -p package_name --all-targets -- -D warnings
```

Before declaring a larger migration stage complete, run:

```bash
cargo fmt --all --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

If a command cannot be run, report why and state what was validated instead.

Do not silently omit failed checks.

## Completion Report

After completing each unit, report:

* Original method or unit.
* Rust method or unit.
* Files changed.
* Whether the implementation is sync or async.
* Rust-specific structural adjustments.
* Tests and checks executed.
* Test results.
* Commit message and hash.
* Known limitations.
* Recommended next migration unit.

Do not claim that a commit exists unless Git confirms it succeeded.

## Definition of Done

A migration unit is complete only when:

* It has a clearly identifiable original counterpart.
* Rust naming and project conventions are followed.
* Async is used where appropriate.
* Pure computation remains synchronous unless justified.
* Inputs, outputs, state transitions, errors, ordering, and side effects are equivalent.
* External formats remain compatible.
* Relevant tests pass.
* Structural differences are documented.
* No unexplained placeholders remain.
* Formatting and static checks pass.
* The unit has been committed atomically.
* The commit contains no unrelated changes.
