# Kimi Code for Rust

**用 Rust 重写的 Kimi Code —— 一款跑在桌面上的 AI 编程智能体。**

不再是命令行，也不依赖 Electron。Kimi Code for Rust 基于 **Tauri 2 + React 19 + Rust 2024** 打造，把完整的 Agent 运行时、会话管理、权限体系全部收进一个轻量的原生桌面应用里，启动快、占用小、体验顺滑。

<p align="center">
  <img src="https://github.com/vnt-dev/kimi-code-rs/blob/master/docs/main-interface.jpg?raw=true" alt="主界面" width="720">
</p>

## 为什么选择它

- 🦀 **纯 Rust 内核**：Agent 主循环、工具执行、权限闸门、上下文压缩全部由 Rust 实现，性能与内存安全兼得。
- 🖥️ **原生桌面体验**：Tauri 2 驱动，告别 Electron 的臃肿；左侧工作区管理多个项目与会话，右侧流式对话一目了然。
- 🔐 **三种权限模式**：`请求审批` 逐步确认、`自动` 交给策略判断、`完全访问` 全速执行——安全与效率由你掌控。
- 🧠 **完整的 Agent 能力**：计划模式、子代理（Subagent/Swarm）、MCP、技能、定时任务、上下文自动压缩，一个不少。
- 🔑 **一键登录**：OAuth 设备授权流程，自动拉起浏览器，扫码或输入设备码即可接入 Kimi 模型。
- 📡 **流式响应**：思考过程与回答实时渲染，Markdown / GFM 完整支持，代码高亮、表格、任务列表都能看。
- 💾 **数据全在本地**：会话记录、项目布局持久化在本机，凭证存放在 `~/.kimi-code/`，隐私自己说了算。

> 本项目是 [MoonshotAI/kimi-code](https://github.com/MoonshotAI/kimi-code) 的 Rust 实现版本。

## 快速上手

环境要求：Rust 工具链（2024 edition）、Node.js LTS、pnpm 10.x，以及对应平台的 [Tauri 依赖](https://v2.tauri.app/start/prerequisites/)。

```shell
git clone https://github.com/vnt-dev/kimi-code-rs.git
cd kimi-code-rs/apps/kimi-code
pnpm install
pnpm tauri dev
```

首次编译需要几分钟。启动后登录 Kimi 账号、打开一个本地项目目录，就可以让 Agent 帮你读代码、改 Bug、跑命令了。

打包发行版：

```shell
pnpm tauri build
```

产物位于 `target/release/bundle`。

## 项目结构

| 目录 | 说明 |
| --- | --- |
| `apps/kimi-code` | Tauri 2 桌面应用（React + Vite 前端 + Rust 壳） |
| `crates/agent-core-v2` | Agent 运行时核心：主循环、工具、权限、会话、MCP、子代理 |
| `crates/oauth` | OAuth 设备流、凭证存储、模型发现 |
| `crates/protocol` | 共享数据模型与通信协议 |
| `crates/transcript` | 会话记录模型、压缩与分页 |
| `crates/minidb` | 嵌入式日志结构键值/文档数据库 |

## 参与开发

```shell
cargo test --workspace          # 跑全部测试
cargo fmt --all --check         # 检查格式
cargo clippy --workspace --all-targets -- -D warnings   # Lint
```

欢迎 Issue 和 PR。如果喜欢这个项目，点个 Star 就是最大的支持！

## 许可证

[MIT License](LICENSE)
