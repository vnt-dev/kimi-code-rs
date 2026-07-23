# kimi-code-protocol

`kimi-code-protocol` contains the shared Rust data contracts for Kimi Code. It
defines the domain models and wire formats used by REST endpoints, agent events,
and the WebSocket control protocol.

The crate is transport-agnostic: it does not start servers, send requests, open
WebSocket connections, or perform other I/O.

## Features

- Core models for sessions, workspaces, messages, tasks, tools, skills,
  approvals, questions, files, and model catalogs.
- REST request, response, query, path, and event payload types.
- Agent event types, including streaming, tool, shell, subagent, compaction,
  task, prompt, and lifecycle events.
- WebSocket protocol v2 control frames, acknowledgements, event envelopes,
  terminal messages, and operation metadata.
- Standard response envelopes, error codes, cursor pagination, request ID
  helpers, and validated ISO date-time values.
- AsyncAPI 3.1 document generation for the WebSocket protocol.
- Serde serialization and deserialization that preserves the protocol's field
  names, discriminants, optional values, explicit `null` values, and open JSON
  payloads.

All APIs are synchronous because they only construct, validate, serialize, or
deserialize in-memory data.

## Usage

Add the crate to a workspace member's `Cargo.toml`:

```toml
[dependencies]
kimi-code-protocol = { path = "../protocol" }
```

Create a successful response envelope:

```rust
use kimi_code_protocol::ok_envelope;

let response = ok_envelope(vec!["kimi-code"], "request-1");

assert_eq!(response.code, 0);
assert_eq!(response.msg, "success");
assert_eq!(response.request_id, "request-1");
```

Generate the default AsyncAPI document:

```rust
use kimi_code_protocol::{AsyncApiDocumentOptions, create_async_api_document};

let document = create_async_api_document(AsyncApiDocumentOptions::default());

assert_eq!(document["asyncapi"], "3.1.0");
```

## Modules

- `approval` and `question`: interactive request and response contracts.
- `message` and `display`: conversation content and structured tool display
  data.
- `session`, `workspace`, `task`, `tool`, and `skill`: core domain resources.
- `events`: typed agent and session event payloads.
- `rest`: REST endpoint DTOs grouped by resource.
- `ws_control`: WebSocket v2 frames and operation definitions.
- `asyncapi`: generation of the WebSocket AsyncAPI document.
- `envelope`, `error_codes`, and `pagination`: common API response types.
- `request_id` and `time`: validated request IDs and timestamps.
- `file`, `fs`, and `model_catalog`: filesystem, file, and model catalog
  contracts.

The crate root re-exports the main public types and functions. Domain modules
remain public for explicit imports when names overlap.

## Validation

```shell
cargo test -p kimi-code-protocol
cargo clippy -p kimi-code-protocol --all-targets -- -D warnings
```
