# kimi-code-transcript

`kimi-code-transcript` is the Rust implementation of
`@moonshot-ai/transcript`. It provides the data model, state reduction, storage,
pagination, and wire protocol support for agent transcripts.

## Features

- Transcript models for turns, steps, frames, tasks, interactions, attachments,
  and todos.
- Incremental operation reduction, text offset validation, and snapshot
  generation.
- Per-agent and session-level stores with change listeners.
- Transcript grade filtering, history reconstruction, and turn pagination.
- View registration and lookup for tools, inputs, and markers.
- Serde-based wire types, events, and input validation.

All operations are in-memory computations. The crate performs no file, network,
or process I/O, so its current API is synchronous.

## Usage

Add the crate to a workspace member's `Cargo.toml`:

```toml
[dependencies]
kimi-code-transcript = { path = "../transcript" }
```

Create a transcript for an agent:

```rust
use kimi_code_transcript::{AgentId, AgentTranscript};

let transcript = AgentTranscript::new(AgentId::from("main"));
let snapshot = transcript.snapshot(None);

assert!(snapshot.items.is_empty());
```

The crate root re-exports all public types and functions. They can also be
imported from their domain modules:

- `model`: core data models and identifier types.
- `ops`: operation definitions and state reduction.
- `store`: per-agent and session-level state stores.
- `granularity`: grade resolution and operation filtering.
- `history`: history message conversion.
- `pagination`: turn pagination.
- `view`: renderer registry.
- `wire`: wire protocol types, events, and validation.

## Validation

```shell
cargo test -p kimi-code-transcript
cargo clippy -p kimi-code-transcript --all-targets -- -D warnings
```
