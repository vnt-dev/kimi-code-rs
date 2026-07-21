# AGENTS.md

## Project Objective

The objective of this project is to migrate the existing project to Rust while preserving its behavior as closely as practical.

The migration should preserve:

* Functional behavior
* Public interfaces
* Method responsibilities
* Method call relationships
* Data structures
* State transitions
* Error conditions
* Input and output formats
* Protocol behavior
* Side effects
* Test results

The preferred migration granularity is the **method level**.

Each meaningful method or function in the original project should normally have a clearly identifiable Rust counterpart.

This is primarily a migration project, but the Rust implementation is expected to follow Rust naming conventions, Rust coding style, Cargo project conventions, ownership rules, error-handling practices, and asynchronous programming conventions.

The goal is not a literal line-by-line translation.

The goal is a method-level, behaviorally equivalent, idiomatic Rust implementation.

## Core Migration Principle

The migration should balance two requirements:

1. Preserve the original behavior and method-level structure.
2. Produce code that is natural, maintainable, and conventional in Rust.

When these requirements conflict, preserve externally observable behavior first.

Rust-specific structural adjustments are allowed when they:

* Do not change business logic
* Do not change external behavior
* Do not change protocol formats
* Do not change error semantics
* Do not make comparison with the original implementation unnecessarily difficult
* Improve safety, readability, maintainability, or compatibility with Rust conventions

## Method-Level Migration

The preferred migration unit is one original method or one tightly related group of methods.

For every significant original method, the Rust implementation should make it possible to determine:

* Which Rust method corresponds to it
* Whether the parameters have the same meaning
* Whether the return value has the same meaning
* Whether the branches correspond to the original branches
* Whether the same state is modified
* Whether external calls happen in an equivalent order
* Whether the same errors can occur
* Whether the method became asynchronous
* Whether Rust language constraints required structural changes

Whenever practical, preserve a one-to-one relationship such as:

```text
Original method                  Rust method
parsePacket()                    parse_packet()
loadConfiguration()              load_configuration().await
updateSessionState()             update_session_state()
sendResponse()                   send_response().await
```

Do not unnecessarily:

* Merge several unrelated original methods into one Rust method
* Split one straightforward original method into many tiny methods
* Move logic across unrelated modules
* Hide original control flow behind complex abstractions
* Replace understandable method-level logic with excessive macros

A method may be split when required for:

* Ownership or borrowing
* Reusable validation
* Async and blocking separation
* Resource lifecycle management
* Error conversion
* Platform abstraction
* Testability
* Rust trait implementations

When splitting a method, keep one primary Rust method that clearly represents the original method whenever possible.

Example:

```rust
pub async fn load_configuration(
    &self,
    path: &Path,
) -> Result<Configuration, ConfigError> {
    let content = self.read_configuration_file(path).await?;
    self.parse_configuration(&content)
}
```

In this example, `load_configuration` remains the primary counterpart of the original method even though I/O and parsing are separated.

## Semantic Equivalence

A one-to-one migration means preserving semantics rather than reproducing individual source lines.

The Rust implementation should preserve:

* The same input-to-output behavior
* The same branch selection
* The same operation dependencies
* The same state transitions
* Equivalent error conditions
* Equivalent side effects
* The same boundary-case behavior
* The same externally visible ordering requirements

Exact statement ordering may be adjusted when the adjustment:

* Is required by ownership or borrowing
* Is required for asynchronous execution
* Does not change observable behavior
* Does not change state-transition semantics
* Does not change synchronization behavior

Do not claim equivalence based only on successful compilation.

## Rust Naming Conventions

All new Rust code must follow standard Rust naming conventions.

Use:

* `snake_case` for functions, methods, variables, modules, and file names
* `UpperCamelCase` for structs, enums, traits, and enum variants
* `SCREAMING_SNAKE_CASE` for constants and statics
* `'a`, `'ctx`, or similarly meaningful names for lifetimes
* Conventional names such as `new`, `default`, `from_*`, `into_*`, `as_*`, and `try_*` where appropriate

Example:

```rust
pub struct SessionManager {
    active_sessions: HashMap<SessionId, Session>,
}

impl SessionManager {
    pub async fn load_session(
        &self,
        session_id: SessionId,
    ) -> Result<Option<Session>, SessionError> {
        // ...
    }
}
```

Do not preserve non-Rust naming forms such as:

```text
loadSession
LoadSession
load_session_data_impl_internal_v2
m_activeSessions
SESSIONmanager
```

unless the name is externally required by:

* FFI
* Serialization
* Protocol definitions
* Generated code
* Public compatibility requirements

External names should be preserved through attributes or adapters instead of violating internal Rust naming conventions.

Example:

```rust
#[derive(serde::Serialize, serde::Deserialize)]
pub struct UserRequest {
    #[serde(rename = "userId")]
    pub user_id: String,
}
```

## Rust Coding Style

The migrated code should be idiomatic Rust when doing so does not change behavior.

Prefer:

* Explicit ownership
* Borrowing instead of unnecessary cloning
* `Result<T, E>` for recoverable errors
* `Option<T>` for optional values
* Pattern matching
* Exhaustive enums
* RAII-based resource management
* Iterators when they remain readable
* Traits for meaningful abstractions
* Small, focused data types
* Newtype wrappers for semantically distinct identifiers
* Standard conversions such as `From`, `TryFrom`, `AsRef`, and `IntoIterator`
* Structured error types
* `async`/`await` for asynchronous I/O

Avoid:

* Mechanical translation of object-oriented patterns
* Getter and setter methods for every field without a semantic reason
* Deep inheritance-like structures
* Excessive trait abstraction
* Excessive generic parameters
* Excessive macros
* Unnecessary dynamic dispatch
* Unnecessary `Arc<Mutex<T>>`
* Large numbers of `clone()` calls
* Large numbers of `unwrap()` calls
* Java-style utility classes
* C-style global mutable state
* Boolean parameters with unclear meaning
* Stringly typed state or error handling

Idiomatic Rust adjustments are allowed when they preserve the original logic.

For example, an original nullable return value may become:

```rust
fn find_session(&self, id: SessionId) -> Option<&Session>
```

An original exception-based result may become:

```rust
fn parse_packet(data: &[u8]) -> Result<Packet, ParseError>
```

An original resource-owning class may become a Rust struct whose resources are released through `Drop`.

## Rust Project Conventions

The project must follow standard Cargo and Rust project conventions.

Typical layout:

```text
Cargo.toml
Cargo.lock
src/
    lib.rs
    main.rs
    module_name.rs
    module_name/
        mod.rs
        submodule.rs
tests/
examples/
benches/
```

Use a workspace when the project naturally contains multiple packages:

```text
Cargo.toml
crates/
    core/
    protocol/
    server/
    client/
```

Do not create unnecessary crates merely to reproduce the original directory hierarchy.

Use crates when there is a meaningful reason, such as:

* Independent reuse
* Separate binaries
* Platform-specific implementations
* Clear dependency boundaries
* Compile-time isolation
* Feature isolation

Keep module visibility as narrow as practical:

```rust
pub
pub(crate)
pub(super)
private
```

Do not make every migrated type and function public.

Public APIs should be intentionally designed and documented.

## Allowed Structural Adjustments

The following adjustments are allowed when they do not change business behavior:

* Renaming identifiers to Rust naming conventions
* Reorganizing imports
* Replacing nullable values with `Option<T>`
* Replacing exception-based control flow with `Result<T, E>`
* Replacing inheritance with composition or traits
* Replacing callbacks with async functions or channels
* Replacing manual cleanup with RAII
* Replacing integer flags with bitflags or enums
* Replacing magic state strings with enums
* Separating blocking and asynchronous operations
* Splitting parsing from I/O
* Introducing private helper methods
* Using newtype wrappers
* Narrowing visibility
* Removing code that exists only because of limitations in the original language

These adjustments must not silently change:

* Inputs
* Outputs
* State transitions
* Error conditions
* Protocol formats
* Timing requirements
* Concurrency guarantees
* Persistence behavior

## Source Mapping

The Rust implementation must remain traceable to the original project.

At module or method boundaries, add source mapping comments when useful:

```rust
// Original:
//   src/session/SessionManager.java
//   SessionManager.loadSession()
```

For a method that has been significantly adapted:

```rust
// Original:
//   src/network/client.c
//   client_send_request()
//
// Rust adaptation:
//   Converted blocking socket I/O to Tokio async I/O.
//   Request construction and response parsing preserve the original behavior.
```

Do not add source comments to every line.

Use them for:

* Significant methods
* Complex control flow
* State machines
* Protocol processing
* Methods whose structure changed substantially
* Behavior that intentionally preserves an original quirk

## Async-First Implementation

Prefer asynchronous Rust for operations that may block or wait.

The Rust project should use an async-first design for:

* Network I/O
* File I/O when an async implementation is available and beneficial
* Database operations
* IPC
* Timers
* Waiting for external processes
* Message queues
* Channels
* Service calls
* Concurrent request handling
* Long-running server operations
* Operations whose original implementation uses callbacks, futures, promises, tasks, or threads primarily for waiting

The default async runtime should be the runtime already selected by the project.

When no runtime has been selected and the project requires general-purpose asynchronous I/O, prefer Tokio unless project constraints indicate otherwise.

Typical asynchronous signatures:

```rust
pub async fn connect(
    &self,
    address: SocketAddr,
) -> Result<Connection, ConnectError>
```

```rust
pub async fn read_packet(
    stream: &mut TcpStream,
) -> Result<Packet, PacketError>
```

```rust
pub async fn save_record(
    &self,
    record: &Record,
) -> Result<(), StorageError>
```

Do not make a method asynchronous merely because asynchronous code is preferred.

Pure computation should normally remain synchronous.

Examples that should usually remain synchronous:

* Parsing an in-memory byte slice
* Calculating a checksum
* Updating a local state structure
* Validating an already loaded value
* Encoding a value into an in-memory buffer
* Comparing records
* Performing a short CPU-only transformation

Example:

```rust
pub fn calculate_checksum(data: &[u8]) -> u16 {
    // CPU-only work remains synchronous.
}
```

A higher-level I/O method may call synchronous computation methods:

```rust
pub async fn receive_packet(
    &mut self,
) -> Result<Packet, ReceiveError> {
    let bytes = self.read_packet_bytes().await?;
    parse_packet(&bytes).map_err(ReceiveError::Parse)
}
```

## Async Method Mapping

When an original method performs blocking I/O, prefer converting it into an async Rust method.

Example mapping:

```text
Original blocking method          Rust async method
readFile()                        read_file().await
connectSocket()                   connect_socket().await
queryDatabase()                   query_database().await
waitForResponse()                 wait_for_response().await
sleepAndRetry()                   sleep_and_retry().await
```

The asynchronous conversion must preserve:

* The same logical operation
* The same result
* The same timeout semantics
* The same retry count
* The same cancellation expectations
* The same state changes
* The same ordering dependencies
* Equivalent cleanup behavior

Do not introduce new concurrency simply because a method becomes asynchronous.

The following:

```rust
let first = first_operation().await?;
let second = second_operation().await?;
```

must not be changed to concurrent execution:

```rust
let (first, second) = tokio::try_join!(
    first_operation(),
    second_operation(),
)?;
```

unless the operations are proven independent and concurrent execution does not change behavior.

Async does not automatically mean concurrent.

## Concurrency Rules

Use concurrency only when:

* The original implementation is concurrent
* The operation is explicitly independent
* Concurrency is required for performance or responsiveness
* The behavior has been verified under concurrent execution

Preserve:

* Ordering guarantees
* Shared-state semantics
* Locking behavior
* Task lifecycle
* Cancellation behavior
* Timeout behavior
* Backpressure behavior
* Maximum concurrency
* Shutdown behavior

Prefer structured concurrency.

Examples include:

```rust
tokio::try_join!
tokio::select!
JoinSet
Semaphore
mpsc
oneshot
watch
broadcast
CancellationToken
```

Choose the primitive that matches the original semantics.

Do not detach tasks without controlling their lifecycle.

Avoid:

```rust
tokio::spawn(async move {
    // Fire-and-forget work with no shutdown or error handling.
});
```

unless the original behavior is explicitly fire-and-forget and task failures are intentionally ignored.

Track background tasks and ensure that:

* Their errors are handled
* They can be cancelled when required
* Shutdown waits for required cleanup
* They do not outlive resources they depend on

## Blocking Work in Async Code

Do not execute blocking operations directly on async runtime worker threads.

Blocking work includes:

* Blocking file APIs
* Blocking socket APIs
* Long CPU-intensive calculations
* Process waits
* Blocking foreign-library calls
* Synchronous database drivers
* Long-held standard mutex operations

When a blocking operation cannot be replaced with an asynchronous implementation, use an appropriate blocking boundary:

```rust
let result = tokio::task::spawn_blocking(move || {
    perform_blocking_operation()
})
.await
.map_err(TaskError::Join)??;
```

Do not wrap every small synchronous calculation in `spawn_blocking`.

Use it only when the operation is sufficiently blocking or CPU-intensive to interfere with the async runtime.

Prefer asynchronous libraries where practical:

```text
Blocking API                     Async alternative
std::net                         tokio::net
std::process                     tokio::process
std::sync::Mutex                 tokio::sync::Mutex when held across await
std::thread::sleep               tokio::time::sleep
blocking database client         async database client
```

The choice between `std::sync::Mutex` and `tokio::sync::Mutex` must depend on lock usage.

Use `std::sync::Mutex` when the critical section is short and does not cross `.await`.

Use `tokio::sync::Mutex` when the lock must intentionally remain held across an `.await`, although designs that avoid holding locks across `.await` are preferred.

## Async Trait Design

Use native async trait methods when supported by the project's minimum Rust version and object-safety requirements.

Example:

```rust
pub trait Storage {
    async fn load(
        &self,
        key: &str,
    ) -> Result<Option<Vec<u8>>, StorageError>;
}
```

Use `async-trait` only when required by compatibility or dynamic-dispatch constraints.

Do not add `async-trait` automatically if native async traits are sufficient.

When dynamic dispatch is required, clearly document any:

* Allocation overhead
* `Send` requirements
* Lifetime limitations
* Object-safety constraints

## Send and Sync Requirements

Do not add `Send + Sync + 'static` constraints mechanically.

Add them when required by:

* Task spawning
* Shared cross-thread ownership
* Runtime executor requirements
* Public API guarantees
* Trait-object usage

Prefer the narrowest valid bounds.

Before using `tokio::spawn`, confirm that captured values satisfy the required ownership and `Send + 'static` constraints.

Use local task execution only when the runtime and application architecture support it intentionally.

## Cancellation Safety

Async methods that may be cancelled must preserve consistent state.

Pay special attention to methods used inside:

```rust
tokio::select!
timeout
task cancellation
connection shutdown
```

A cancellable method must not leave:

* Partially updated shared state
* Corrupted protocol buffers
* Unreleased logical resources
* Half-written persistent records
* Unclear transaction state

When an operation must complete atomically, use an explicit transaction, state guard, temporary buffer, or non-cancellable critical section.

Document methods that are not cancellation-safe.

## Timeout and Retry Behavior

Preserve original timeout and retry behavior.

Explicitly define:

* Timeout duration
* Whether timeout covers one attempt or the entire operation
* Maximum attempt count
* Delay between attempts
* Backoff strategy
* Which errors are retryable
* Whether state is reset between attempts
* What error is returned after exhaustion

Example:

```rust
pub async fn connect_with_retry(
    &self,
    address: SocketAddr,
) -> Result<Connection, ConnectError> {
    for attempt in 0..MAX_ATTEMPTS {
        match tokio::time::timeout(
            CONNECT_TIMEOUT,
            self.connect_once(address),
        )
        .await
        {
            Ok(Ok(connection)) => return Ok(connection),
            Ok(Err(error)) if error.is_retryable() => {
                if attempt + 1 < MAX_ATTEMPTS {
                    tokio::time::sleep(RETRY_DELAY).await;
                }
            }
            Ok(Err(error)) => return Err(error),
            Err(_) if attempt + 1 < MAX_ATTEMPTS => {
                tokio::time::sleep(RETRY_DELAY).await;
            }
            Err(_) => return Err(ConnectError::Timeout),
        }
    }

    Err(ConnectError::RetryExhausted)
}
```

Do not improve or modify the retry algorithm during migration unless explicitly requested.

## Data Structure Migration

Preserve the semantic meaning of original data structures.

Use Rust types that express the original constraints safely.

Typical mappings:

```text
Original concept                Rust representation
Nullable value                  Option<T>
Recoverable operation           Result<T, E>
Dynamic array                   Vec<T>
Byte buffer                     Vec<u8>, Bytes, BytesMut
Read-only bytes                 &[u8], Bytes
Map                             HashMap<K, V>, BTreeMap<K, V>
Set                             HashSet<T>, BTreeSet<T>
Shared immutable ownership      Arc<T>
Single-thread shared ownership  Rc<T>
Shared mutable state            Arc<Mutex<T>>, Arc<RwLock<T>>
Fixed-size value                [T; N]
Distinct identifier             Newtype struct
Finite state                    enum
Bit flags                       bitflags
```

Select integer types based on:

* Original bit width
* Signedness
* Protocol representation
* File format
* Database representation
* Overflow semantics
* Platform dependence

Do not use `usize` for protocol or persisted integer fields unless the format is explicitly platform-sized.

## Object-Oriented Source Projects

Do not mechanically reproduce class inheritance.

Translate concepts according to Rust conventions:

```text
Original concept                Rust approach
Data class                      struct
Closed set of states            enum
Interface                       trait
Inheritance for reuse           composition
Inheritance for polymorphism    trait or enum dispatch
Static utility class            module functions
Constructor                     new() or builder
Destructor                      Drop
Nullable reference              Option<&T> or Option<Arc<T>>
```

Preserve method-level responsibilities even when the type structure changes.

When an original class contains unrelated responsibilities, do not perform a broad redesign during initial migration unless necessary.

Small Rust-oriented separations are allowed when behavior remains clear and the original method mapping remains traceable.

## Function and Method Signatures

Rust signatures should follow Rust conventions rather than mechanically preserving original parameter forms.

Prefer:

```rust
pub fn parse(data: &[u8]) -> Result<Packet, ParseError>
```

instead of:

```rust
pub fn parse(data: &Vec<u8>) -> Result<Packet, ParseError>
```

Prefer:

```rust
pub fn name(&self) -> &str
```

instead of returning an unnecessary cloned `String`.

Prefer:

```rust
pub async fn save(&self, value: &Value) -> Result<(), SaveError>
```

when the method performs asynchronous I/O.

Use ownership intentionally:

* `T` when the method consumes the value
* `&T` for read-only borrowing
* `&mut T` for exclusive mutation
* `Arc<T>` for shared ownership
* `Cow<'_, T>` only when both borrowed and owned data are genuinely useful

Do not overcomplicate lifetimes merely to eliminate a harmless clone.

However, do not use cloning as the default solution to every borrowing issue.

## Error Handling

Use structured Rust error handling.

Prefer dedicated error enums:

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

* Error conditions
* Error categories
* Retryability
* Error propagation
* Externally visible messages when required
* Error codes when part of an API or protocol

Avoid:

```rust
unwrap()
expect()
panic!()
todo!()
unimplemented!()
```

in production paths unless the original behavior is intentionally unrecoverable.

Do not convert all errors into generic strings.

Use `anyhow` primarily at application boundaries where callers do not need to match specific error variants.

Use typed errors in libraries and domain logic.

## Control Flow

Preserve the original logical control flow while expressing it clearly in Rust.

Using `match`, `if let`, `while let`, iterators, and the `?` operator is encouraged when it does not obscure the original behavior.

Example:

```rust
let session = self
    .sessions
    .get(&session_id)
    .ok_or(SessionError::NotFound(session_id))?;
```

Do not replace straightforward logic with deeply nested iterator combinators merely for stylistic reasons.

Readable and reviewable code takes priority over maximum concision.

## Serialization and Protocol Compatibility

External formats must remain compatible.

Preserve:

* Field names
* Field types
* Optional fields
* Null values
* Default values
* Unknown-field behavior
* Enum representations
* Numeric precision
* Time formats
* Time units
* Byte order
* Length encoding
* Alignment where externally relevant
* Error codes
* Message ordering where required

Use attributes to preserve external names:

```rust
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequest {
    pub session_id: String,

    #[serde(default)]
    pub force_reload: bool,
}
```

Internal Rust naming should remain idiomatic even when external names are not.

For binary protocols, add byte-for-byte compatibility tests.

## Numeric Behavior

Preserve:

* Signedness
* Bit width
* Overflow behavior
* Underflow behavior
* Rounding
* Truncation
* Floating-point precision
* Shift semantics
* Endianness
* NaN behavior
* Infinity behavior
* Division-by-zero behavior

Use explicit arithmetic operations when semantics matter:

```rust
wrapping_add
checked_add
saturating_add
overflowing_add
```

Do not depend accidentally on debug-versus-release overflow differences.

## Resource Management

Use RAII for:

* Files
* Sockets
* Locks
* Database transactions
* Temporary files
* Buffers
* Foreign handles
* Background task guards

When explicit shutdown or flushing is behaviorally required, do not rely only on `Drop`.

Example:

```rust
pub async fn shutdown(&mut self) -> Result<(), ShutdownError> {
    self.flush_pending_data().await?;
    self.close_transport().await?;
    Ok(())
}
```

Preserve the original cleanup order when it is externally significant.

## Unsafe Code

Safe Rust is required by default.

Use `unsafe` only when necessary for:

* FFI
* Operating-system APIs
* Raw protocol memory
* Memory-mapped hardware
* Performance requirements demonstrated by measurement
* Behavior that cannot reasonably be represented in safe Rust

Every `unsafe` block must:

* Be as small as practical
* Include a `SAFETY` comment
* State its invariants
* Be wrapped by a safe API where practical
* Have relevant tests

Do not use `unsafe` merely to bypass ownership or borrowing design.

## Dependencies

Prefer stable, maintained, widely used crates.

Before adding a dependency, verify:

* Whether the standard library is sufficient
* Whether the project already has a suitable dependency
* Whether the crate supports the target platforms
* Whether the crate is compatible with the selected async runtime
* Whether the dependency materially simplifies or improves the implementation
* Whether it introduces an unnecessary runtime or large dependency tree

Do not replace core behavior simply because a crate offers a different implementation.

Record important source-to-Rust dependency mappings.

## Temporary Implementations

Do not silently add:

* Empty functions
* Fake return values
* Placeholder data
* Ignored errors
* Unimplemented branches
* Production mocks
* Incomplete platform logic

Mark temporary work explicitly:

```rust
// MIGRATION-TODO:
// Original: src/network/Client.java, Client.connect()
// Missing dependency: TLS transport has not been migrated
// Temporary behavior: returns ConnectError::TlsUnavailable
// Completion condition: finish the TLS transport migration
```

Temporary implementations must never be reported as complete.

## Testing Requirements

Tests are the primary evidence of behavioral equivalence.

For every migrated method or method group, add or migrate relevant tests.

Cover:

* Normal behavior
* Empty input
* Invalid input
* Boundary values
* Minimum and maximum values
* Error paths
* State transitions
* Repeated calls
* Timeout behavior
* Retry behavior
* Cancellation behavior
* Concurrent behavior
* Serialization
* Binary compatibility
* Resource cleanup
* Original implementation quirks

When practical, run the same test vectors against both implementations.

Compare:

* Return values
* Errors
* Output data
* State changes
* Generated files
* Network messages
* Serialized bytes
* Database changes
* Logs when externally significant

For asynchronous code, use deterministic test techniques where possible:

```rust
#[tokio::test]
async fn loads_existing_session() {
    // ...
}
```

Use paused or controlled time for timeout and retry tests when supported.

Avoid tests that depend unnecessarily on real sleep durations or external networks.

## Migration Workflow

For each migration unit:

1. Read the original method and its callers.
2. Read the original tests.
3. Identify inputs, outputs, state changes, side effects, and errors.
4. Determine whether the Rust method should be synchronous or asynchronous.
5. Define the idiomatic Rust signature.
6. Implement the method while preserving behavior.
7. Add or migrate tests.
8. Compare behavior with the original implementation.
9. Run formatting and targeted validation.
10. Review the diff.
11. Commit the migration unit.
12. Start the next unit only after the current unit has been committed.

Prefer method-level units.

A commit may contain several methods only when they are tightly coupled and cannot be meaningfully compiled, tested, or reviewed separately.

## Implement One Unit, Commit One Unit

Complete and commit one coherent migration unit before beginning the next independent unit.

The normal workflow is:

```text
Implement method or method group A
Test method or method group A
Review the diff
Commit A

Implement method or method group B
Test method or method group B
Review the diff
Commit B
```

Do not use this workflow:

```text
Implement many unrelated methods
Modify several modules
Run all tests at the end
Create one large migration commit
```

A migration unit may be:

* One method
* One tightly coupled method group
* One struct and its core methods
* One enum and its state-transition methods
* One protocol message and its parser or encoder
* One asynchronous I/O operation
* One error type and its direct integration
* One test fixture required by the current method
* One small module when its methods cannot reasonably be separated

Each unit should be independently understandable, testable, reviewable, and revertible.

## Commit Boundary Requirements

Before committing a unit:

* The code must be formatted.
* The relevant package must compile.
* Relevant tests must pass.
* Relevant Clippy checks should pass.
* No unexplained placeholder behavior may be present.
* The staged files must belong to the same migration unit.
* Unrelated user changes must remain untouched.
* The commit must not depend on unstaged local modifications.
* The commit must preserve all previously completed behavior.

Prefer explicit staging:

```bash
git add src/session.rs tests/session_tests.rs
```

Avoid blindly staging the entire working tree:

```bash
git add .
```

unless every change has been reviewed and belongs to the current unit.

Before committing, inspect:

```bash
git status --short
git diff
git diff --check
git diff --cached
```

## Commit Message Format

Use clear English commit messages.

Preferred format:

```text
migrate: port <method or unit> to Rust
```

Examples:

```text
migrate: port packet parsing to Rust
migrate: port session lookup to Rust
migrate: port async socket connection to Rust
migrate: port configuration loading to Rust
migrate: port retransmission timeout handling to Rust
```

For tests:

```text
test: add parity tests for packet parsing
```

For required project infrastructure:

```text
build: add Tokio runtime support
build: add async database dependency
chore: add migration test fixtures
```

Do not use vague messages such as:

```text
update code
fix things
migration
work in progress
misc changes
```

## Commit Description

For nontrivial commits, include:

* Original file and method
* Rust file and method
* Behavior migrated
* Rust-specific adjustments
* Async conversion details
* Tests executed
* Known limitations

Example:

```text
migrate: port async session loading to Rust

Original:
- src/session/SessionManager.java
- SessionManager.loadSession()

Rust:
- src/session/manager.rs
- SessionManager::load_session()

Changes:
- Converted blocking repository access to async/await.
- Preserved cache lookup order and not-found behavior.
- Replaced nullable return value with Option<Session>.
- Replaced exceptions with typed SessionError variants.

Tests:
- cargo test session::manager::tests

Known limitation:
- Distributed cache integration is migrated separately.
```

## Git Safety Rules

Do not:

* Rewrite shared history
* Force-push
* Rebase shared branches
* Amend shared commits
* Squash existing commits without instruction
* Delete existing commits
* Reset or discard user changes
* Commit secrets
* Commit generated binaries
* Commit editor files
* Commit unrelated formatting changes

Do not run the following without explicit instruction:

```bash
git reset --hard
git clean -fd
git push --force
git push --force-with-lease
git rebase
git commit --amend
```

If unrelated uncommitted changes exist, leave them untouched and stage only the current migration files.

## Build and Validation

Before committing a migration unit, run relevant targeted checks.

Examples:

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

If a command cannot be run, report:

* Which command was skipped
* Why it was skipped
* What validation was performed instead

Do not silently omit failed checks.

## Completion Report

After each completed unit, report:

* Original method or unit
* Rust method or unit
* Files added or changed
* Whether the implementation is sync or async
* Rust-specific structural adjustments
* Tests executed
* Test results
* Commit message
* Commit hash
* Known limitations
* Next recommended migration unit

Example:

```text
Completed unit:
- SessionManager.loadSession()

Rust implementation:
- SessionManager::load_session().await

Files:
- src/session/manager.rs
- tests/session_manager.rs

Adjustments:
- Renamed methods and fields to Rust naming conventions.
- Converted blocking storage access to async.
- Replaced nullable result with Option<Session>.
- Replaced exceptions with SessionError.

Validation:
- cargo fmt --all --check
- cargo test session_manager
- cargo clippy --all-targets -- -D warnings

Commit:
- 3a9f812 migrate: port async session loading to Rust

Known limitations:
- Remote cache fallback is not included.

Next unit:
- SessionManager.saveSession()
```

Do not claim a commit was created unless Git confirms it succeeded.

## Prohibited Actions

Do not:

* Translate code without reading the original method.
* Guess behavior from method or variable names.
* Perform an uncontrolled full-project rewrite.
* Change business rules for a more elegant Rust design.
* Change protocol formats.
* Change default values.
* Change error semantics silently.
* Change execution order when order is observable.
* Introduce concurrency without proving operations are independent.
* Convert pure calculations to async without a reason.
* Hold blocking operations on async runtime threads.
* Spawn unmanaged background tasks.
* Hold locks across `.await` without justification.
* Hide incomplete work behind default values.
* Present placeholder logic as completed.
* Fix original bugs silently.
* Mix unrelated migration units in one commit.
* Start the next independent unit before committing the completed unit.
* Modify or discard unrelated user changes.
* Claim full equivalence without test evidence.

## Definition of Done

A migration unit is complete only when:

* The original method or unit has a clearly identifiable Rust counterpart.
* The Rust code follows Rust naming conventions.
* The Rust code follows Rust coding conventions.
* The code fits the Cargo project structure.
* Async is used where waiting or blocking I/O makes it appropriate.
* Pure computation remains synchronous unless there is a concrete reason otherwise.
* Inputs and outputs are equivalent.
* State transitions are equivalent.
* Error conditions are equivalent.
* External formats remain compatible.
* Relevant tests pass.
* Rust-specific structural differences are documented.
* No unexplained placeholder logic remains.
* Formatting and static checks pass.
* The unit has been committed in an atomic Git commit.
* The commit contains no unrelated changes.

## Conflict Resolution Priority

When requirements conflict, apply the following priority:

1. Preserve externally observable behavior.
2. Preserve protocol and data-format compatibility.
3. Preserve state-transition and error semantics.
4. Preserve method-level traceability.
5. Satisfy Rust memory-safety requirements.
6. Preserve concurrency, timeout, and cancellation semantics.
7. Follow Rust project and naming conventions.
8. Use idiomatic Rust abstractions.
9. Prefer asynchronous I/O.
10. Optimize performance only after behavioral equivalence has been established.

## Final Review Questions

Every migrated method or unit should make it possible to answer:

1. Which original method does this Rust method represent?
2. Are its inputs, outputs, errors, and side effects equivalent?
3. Was the method renamed according to Rust conventions?
4. Was the method made async, and was async appropriate?
5. Did asynchronous conversion change ordering or concurrency?
6. Did Rust ownership or borrowing require structural changes?
7. Are those changes behaviorally neutral?
8. Which tests demonstrate equivalence?
9. Which Git commit introduced the unit?
10. Can the commit be reviewed and reverted independently?
11. Does the commit contain only the intended migration scope?
12. Does the implementation contain any placeholder or unverified behavior?

If these questions cannot be answered, the migration unit must not be considered complete.
