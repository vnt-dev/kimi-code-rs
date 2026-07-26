# Kimi Code for Rust

Kimi Code for Rust is a native desktop coding agent built with Rust, Tauri,
React, and TypeScript. The desktop client uses `kimi-code-agent-core-v2` for
authentication, model discovery, agent execution, tool interactions, and
conversation context management.

## Features

- Organize local workspaces as projects with multiple conversations.
- Sign in to Kimi Code through the OAuth device flow, with automatic browser
  opening and a visible fallback code.
- Discover available models and select model-specific reasoning effort.
- Stream assistant reasoning and responses into the conversation.
- Render assistant messages as Markdown with GitHub Flavored Markdown support.
- Review command and tool requests directly in the client.
- Choose between three permission modes:
  - **Request Approval** asks before protected operations.
  - **Auto** lets the permission policy decide whether an operation can run.
  - **Full Access** bypasses approval and executes operations immediately.
- Display automatic and manual context-compaction lifecycle events.
- Persist the desktop project and conversation layout locally.

> [!CAUTION]
> Full Access allows the agent to execute commands without approval. Use it only
> in workspaces you trust and understand.

## Repository Layout

| Path | Purpose |
| --- | --- |
| `apps/kimi-code` | Tauri 2 desktop application with a React and Vite frontend |
| `crates/agent-core-v2` | Agent runtime, sessions, tools, providers, permissions, and desktop facade |
| `crates/oauth` | OAuth device flow, credential storage, model discovery, and managed authentication |
| `crates/protocol` | Shared domain models and wire-format definitions |
| `crates/transcript` | Transcript models, reduction, history, and pagination |
| `crates/minidb` | Embedded log-structured key-value and document database |

## Prerequisites

Install the following before building the project:

- A stable Rust toolchain that supports Rust 2024 edition
- A current Node.js LTS release
- [pnpm](https://pnpm.io/) 10.x
- The platform-specific [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)

On Windows, Tauri also requires the Microsoft C++ Build Tools and WebView2
components described in the prerequisites above.

## Getting Started

Clone the repository:

```shell
git clone https://github.com/vnt-dev/kimi-code-rs.git
cd kimi-code-rs
```

Install the desktop frontend dependencies:

```shell
cd apps/kimi-code
pnpm install
```

Start the desktop application in development mode:

```shell
pnpm tauri dev
```

The first Rust build can take several minutes because Cargo must compile the
Tauri application and the agent runtime.

## Building

Create a production desktop bundle from `apps/kimi-code`:

```shell
pnpm tauri build
```

Installers and bundles are written under `target/release/bundle` when Cargo
uses the workspace's default target directory.

## Development

Build the frontend:

```shell
cd apps/kimi-code
pnpm build
```

Run the Rust test suite from the repository root:

```shell
cargo test --workspace
```

Check formatting and lints:

```shell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Individual crates can also be tested by package name:

```shell
cargo test -p kimi-code-agent-core-v2
cargo test -p kimi-code-minidb
cargo test -p kimi-code-oauth
cargo test -p kimi-code-protocol
cargo test -p kimi-code-transcript
```

## Local Data and Credentials

The desktop shell stores its project list, conversation list, and visible
message history in the WebView's local storage. Kimi Code credentials are
stored under `~/.kimi-code/credentials` by default. Do not commit credentials,
access tokens, refresh tokens, or exported conversation data.

The selected project directory becomes the agent's working directory. Commands
and file operations therefore run against that local workspace according to
the active permission mode.

## License

This project is licensed under the [MIT License](LICENSE).
