<p align="center">
  <img src="apps/desktop/src-tauri/icons/icon.png" alt="ExAgent app icon" width="96" height="96">
</p>

<h1 align="center">ExAgent</h1>

<p align="center">
  面向本地编码代理的桌面工作台：项目、持久会话、工具审批、子代理、目标管理和实时运行时检查都在一个 GUI 里。
</p>

<p align="center">
  <a href="README.md">English</a> | 简体中文
</p>

<p align="center">
  <img src="docs/assets/exagent-desktop-chat.png" alt="ExAgent 桌面 GUI，展示运行中的聊天会话、输入框、目标控制和运行时检查器" width="1200">
</p>

## 这是什么

ExAgent 是一个桌面优先的代理工作台，底层是 Rust 运行时，桌面应用使用 Tauri 和 React 构建。它面向本地项目里的长时间编码工作：启动会话、选择 provider/model、审批工具动作、检查运行时状态，并在之后从本地持久历史中恢复线程。

它不只是一个聊天 UI。ExAgent 包含可恢复代理工作的运行时能力：事件回放、持久 shell 会话、带审批的工具、子代理、目标跟踪、项目记忆、MCP 工具、workflow runs，以及用于观察代理行为的桌面检查器。

## 亮点

- 面向编码项目的本地桌面代理工作台
- 可从本地项目历史重新打开的持久会话
- 用 GUI 配置 API key 和 OAuth 模型 provider
- 带审批的编码工具、实时 transcript 和事件检查
- 项目记忆，支持自动 recall、显式 memory tools、本地治理和 audit state
- 持久 shell、子代理、目标、MCP 工具、workflows 和 `SKILL.md` 支持

## 下载

macOS 构建发布在 [GitHub Releases](https://github.com/exqqstar/ExAgent/releases) 页面。

macOS 用户下载 universal DMG，打开后把 ExAgent 拖进 Applications 即可。发布构建已经使用 Developer ID 签名并完成 notarization。

## 快速开始

### 前置条件

- Rust toolchain
- Node.js 和 npm
- 一个你愿意在本机使用的模型 provider 凭据

### 启动桌面应用

```bash
cd apps/desktop
npm ci
npm run tauri:dev
```

桌面应用会启动 Tauri shell 和 Vite 前端。日常使用时，可以直接在 GUI 中配置 providers、projects 和 sessions。

### 配置 provider

打开 **Settings** -> **Providers**，添加 API-key provider，或完成 OAuth 流程。凭据由桌面应用保存在本机。建议使用专门的项目凭据，并保护好本地 app data。

### 添加项目并启动会话

在侧边栏添加一个本地 workspace 目录，创建新 session，在 composer 中输入内容并提交 turn。当 ExAgent 需要执行命令或修改文件的审批时，应用会在 transcript 里显示 approval card。

更完整的操作 walkthrough 见 [docs/demo/exagent-walkthrough.md](docs/demo/exagent-walkthrough.md)。

## 架构概览

ExAgent 围绕本地 Rust runtime 组织，并通过 typed app-server boundary 暴露给桌面端。Tauri shell 负责项目感知的桌面入口，runtime 负责 thread 执行、模型调用、工具、状态和实时事件。

- 每个 thread 都运行在 actor-backed `ThreadRuntime` 后面，因此同一 thread 的 turns 会被串行化，同时 snapshots、status 和 events 会流回 GUI。
- `ThreadSession` 装配一个 thread 的长期运行部件：agent、context、rollout storage、tools、goals、memory、policy 和 execution sessions。
- context 层把真实 conversation history 和 prompt-only internal context 分开；memory recall、goal state、skills、project docs 等只进入 prompt，compaction 可以把长历史替换成结构化摘要。
- 本地持久化是 append-first：每个 thread 有自己的 `rollout.jsonl` ledger，`IndexDb` 存 projects、threads、goals、memory 和 review state 的跨线程索引。
- tool system 把 `src/tools` 里的公开 tool contracts 和 `src/runtime/tool` 里的 per-turn runtime orchestration 分开；agent policy 同时约束工具可见性和执行权限。
- memory system 支持自动 prompt recall、显式 memory tools、candidate saves、本地 promote/archive/forget 流程和 audit state。
- model 层把不同 provider 的 API 统一成 ExAgent 内部的 conversation、tool-call、multimodal、reasoning 和 streaming 类型。
- workflow runtime 以 phase-based scheduler 的方式运行 deep search 等结构化任务，它和普通 chat turn loop 是平行的执行模型。

## 开发

在 `apps/desktop` 下常用命令：

```bash
npm ci
npm run tauri:dev
npm test
npm run build
```

在仓库根目录下常用命令：

```bash
cargo test --package exagent --locked
cargo test --package exagent-desktop --locked
cargo fmt --all -- --check
cargo clippy --package exagent --all-targets
cargo deny check licenses sources bans
```

## 项目状态

ExAgent 仍处于早期阶段，当前主要面向个人工作站上的本地优先使用场景，而不是托管式多人服务。

当前非目标：

- 不提供生产级 OS sandbox 隔离
- 不提供托管协作服务
- 还没有稳定的公开 SDK

## 仓库结构

- [apps/desktop](apps/desktop)：Tauri 桌面 shell 和 React 工作台
- [apps/desktop/src-tauri](apps/desktop/src-tauri)：桌面 Rust commands、settings、provider auth 和 Tauri entrypoint
- [src/app_server](src/app_server)：typed desktop/runtime boundary、request processors、live views 和 projections
- [src/runtime](src/runtime)：实时执行内核、thread actor、session turn loop、agent sampling、tool runtime、policy 和 exec sessions
- [src/runtime/agent_profile](src/runtime/agent_profile)：agent role catalog 和 capability policy
- [src/runtime/goal](src/runtime/goal)：structured goal state、accounting 和 continuation effects
- [src/runtime/memory](src/runtime/memory)：runtime memory bridge，负责接入 context 和 tools
- [src/runtime/workflow](src/runtime/workflow)：structured workflow 和 deep search runtime
- [src/tools](src/tools)：tool trait、registry 和内置编码工具
- [src/state](src/state)：持久 rollout model、桌面 index storage 和 memory state
- [src/model](src/model)：模型 provider adapters 和 conversation types
- [src/mcp](src/mcp)：MCP configuration 和 tool integration
- [tests](tests)：runtime、protocol、policy、tools 和 storage 的集成测试
- [docs/demo](docs/demo)：桌面优先 walkthrough

## 贡献

开发设置、验证命令和 PR 要求见 [CONTRIBUTING.md](CONTRIBUTING.md)。

请不要把 secrets 放进 issues、pull requests、rollout files 或 logs。漏洞报告请使用 [SECURITY.md](SECURITY.md) 中的流程。

## 第三方声明

依赖许可证策略和外部参考材料规则见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

## 作者和声明

ExAgent 由 exqqstar 创建。作者和贡献归属见 [AUTHORS.md](AUTHORS.md)，发行声明见 [NOTICE](NOTICE)。

## 许可证

Copyright (c) 2026 exqqstar.

你可以任选以下许可证使用本项目：

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))
