# kimi-code-agent-core-v2

`kimi-code-agent-core-v2` is the core agent runtime for Kimi Code. It
coordinates model requests, agent loops, context management, tool execution,
permission control, session services, and persistence.

## Origin and Upstream Baseline

This module is a Rust port and adaptation of `agent-core-v2` from
[MoonshotAI/kimi-code](https://github.com/MoonshotAI/kimi-code).

The upstream code included in this implementation is current through commit
[`e45832398d0d9cad98dbad1cbf1e5b103a20aace`](https://github.com/MoonshotAI/kimi-code/commit/e45832398d0d9cad98dbad1cbf1e5b103a20aace).
Features added or changed upstream after that commit are not included unless
they have been explicitly ported to this repository.

Because of differences in language, asynchronous runtime, and project
structure, this module is not a line-by-line translation of the upstream
TypeScript API. Some interfaces, dependency-injection patterns, and internal
implementations have been adapted to Rust's type system and ecosystem.

## Features

- Agent lifecycle, execution loops, and task orchestration
- Model provider abstractions, request handling, and streaming responses
- Conversation context management, compaction, and memory
- Built-in tools, MCP tools, and tool-call management
- Permission modes, rule matching, and workspace access control
- Main-agent, subagent, and swarm coordination
- Session initialization, metadata, logging, and persistence
- Skills, plugins, scheduled tasks, and external hooks
- Bootstrap, authentication, and session APIs for desktop clients

## Project Structure

| Path | Description |
| --- | --- |
| `src/_base` | Dependency injection, lifecycle, logging, execution environment, and shared infrastructure |
| `src/agent` | Agent loops, context, permissions, tools, tasks, and model requests |
| `src/app` | Application bootstrap, authentication, configuration, desktop client, and workspace management |
| `src/kosong` | Model, message protocol, and provider abstractions |
| `src/os` | Interfaces and implementations for operating-system capabilities |
| `src/persistence` | Blob, append-log, query, and storage abstractions |
| `src/session` | Session-scoped services, including agents, MCP, terminals, skills, and subagents |
| `src/tool` | Tool contracts, argument validation, result construction, and path-access policies |
| `src/wire` | Persisted and replayable agent state and event records |
| `examples` | Login, model-listing, and interactive agent examples |

## Build and Test

Run the following commands from the repository root:

```shell
cargo build -p kimi-code-agent-core-v2
cargo test -p kimi-code-agent-core-v2
```

Run the examples:

```shell
# Sign in with OAuth
cargo run -p kimi-code-agent-core-v2 --example login

# List available models
cargo run -p kimi-code-agent-core-v2 --example list_models

# Start an interactive agent
cargo run -p kimi-code-agent-core-v2 --example agent_app -- "Your prompt"
```

The `list_models` and `agent_app` examples normally require authentication
first. The agent may read or modify workspace files and execute local commands,
so select an appropriate permission mode for your environment.

## License

This module is distributed under the repository's
[MIT License](../../LICENSE).
