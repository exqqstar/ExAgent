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

它不只是一个聊天 UI。ExAgent 包含可恢复代理工作的运行时能力：事件回放、持久 shell 会话、带审批的工具、子代理、目标跟踪、过程记忆、MCP 工具，以及用于观察代理行为的桌面检查器。

## 亮点

- 面向编码项目的本地桌面代理工作台
- 可从本地项目历史重新打开的持久会话
- 用 GUI 配置 API key 和 OAuth 模型 provider
- 带审批的编码工具、实时 transcript 和事件检查
- 持久 shell、子代理、目标、MCP 工具和 `SKILL.md` 支持

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
- [src/runtime](src/runtime)：实时执行内核、thread actor、session turn loop、agent sampling、tool runtime、policy 和 exec sessions
- [src/tools](src/tools)：tool trait、registry 和内置编码工具
- [src/state](src/state)：持久 rollout model 和桌面 index storage
- [src/model](src/model)：模型 provider adapters 和 conversation types
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
