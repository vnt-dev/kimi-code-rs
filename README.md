# Kimi Code for Rust

[简体中文](README.zh-CN.md) | English

**A Rust rewrite of Kimi Code — fully compatible with the official Kimi CLI, with both desktop and Web access.**

Kimi Code for Rust is built with **Tauri 2 + React 19 + Rust 2024**. It is fully compatible with the capabilities and data of the official Kimi CLI, while bringing the Agent runtime, conversation management, and permission system into a lightweight native desktop application. Enable the built-in Web server to access it directly from a browser and view or control the same conversation in real time from both desktop and Web clients.

<p align="center">
  <img src="https://github.com/vnt-dev/kimi-code-rs/blob/master/docs/kimi-code-desktop-mobile-showcase-en.png?raw=true" alt="Kimi Code desktop and mobile Web interfaces" width="720">
</p>

## Why Kimi Code for Rust?

- 🦀 **Pure Rust core**: The Agent loop, tool execution, permission gates, and context compaction are all implemented in Rust for high performance and memory safety.
- 🖥️ **Native desktop experience**: Powered by Tauri 2 without Electron's overhead. Manage multiple projects and conversations in the sidebar while following streaming responses in the main view.
- 🌐 **Desktop and Web access**: Enable the built-in Web server to access Kimi Code from a computer or mobile browser, with the same conversation synchronized with the desktop client in real time.
- 🔐 **Three permission modes**: `Request Approval` confirms protected operations, `Auto` lets the policy decide, and `Full Access` runs without approval—putting you in control of safety and speed.
- 🧠 **Complete Agent capabilities**: Plan mode, Subagents/Swarm, MCP, skills, scheduled tasks, and automatic context compaction are all included.
- 🔑 **One-click sign-in**: The OAuth device authorization flow opens your browser automatically, so you can connect to Kimi models by scanning a QR code or entering a device code.
- 📡 **Streaming responses**: Reasoning and answers render in real time, with full Markdown/GFM support for code, tables, task lists, and more.
- 💾 **Local-first data**: Conversation history and project layouts stay on your machine, while credentials are stored under `~/.kimi-code/`, keeping your data under your control.

> This project is a Rust implementation of [MoonshotAI/kimi-code](https://github.com/MoonshotAI/kimi-code).

## Quick Start

Requirements: a Rust toolchain with Rust 2024 edition support, Node.js LTS, pnpm 10.x, and the platform-specific [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).

```shell
git clone https://github.com/vnt-dev/kimi-code-rs.git
cd kimi-code-rs/apps/kimi-code
pnpm install
pnpm tauri dev
```

The first build may take a few minutes. Once the application starts, sign in to your Kimi account and open a local project directory. The Agent can then help you read code, fix bugs, and run commands.

To build a release bundle:

```shell
pnpm tauri build
```

Build artifacts are written to `target/release/bundle`.

Tagged releases build Windows installers, Linux AppImage and Debian (`.deb`)
packages, and macOS disk images for both Apple Silicon and Intel Macs. The
in-app updater is available on all three platforms; Linux automatic updates
require running the AppImage distributed on the GitHub release page.

## Project Structure

| Path | Description |
| --- | --- |
| `apps/kimi-code` | Tauri 2 desktop application (React + Vite frontend and Rust shell) |
| `crates/agent-core-v2` | Agent runtime core: loop, tools, permissions, conversations, MCP, and Subagents |
| `crates/oauth` | OAuth device flow, credential storage, and model discovery |
| `crates/protocol` | Shared data models and communication protocols |
| `crates/transcript` | Conversation transcript models, compaction, and pagination |
| `crates/minidb` | Embedded log-structured key-value/document database |

## Contributing

```shell
cargo test --workspace          # Run the full test suite
cargo fmt --all --check         # Check formatting
cargo clippy --workspace --all-targets -- -D warnings   # Run lints
```

Issues and pull requests are welcome. If you enjoy the project, giving it a Star is the best way to show your support!

## License

[MIT License](LICENSE)
