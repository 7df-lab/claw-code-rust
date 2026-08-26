<div align="center">

<img src="./.github/assets/devo-readme-brand.svg" alt="Devo desktop coding agent app icon and wordmark" width="360" />

</div>

<div align="center">

**Devo 是开源 coding agent，提供 Desktop app、终端 TUI/CLI 和模型中立的 Rust runtime，面向私有化、企业内网和 OpenAI 兼容模型环境。可接入 DeepSeek、Qwen、Kimi、Anthropic 兼容 API、本地网关或自定义模型端点。**

[![Stars](https://img.shields.io/github/stars/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/stargazers)
[![Language](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/7df-lab/devo/pulls)
[![CI](https://img.shields.io/github/actions/workflow/status/7df-lab/devo/ci.yml?branch=main&style=flat-square)](https://github.com/7df-lab/devo/actions)
[![Release](https://img.shields.io/github/v/release/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/releases)

[English](./README.md) | [简体中文](./README.zh-Hans.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | [Русский](./README.ru.md)

[为什么选择 Devo](#为什么选择-devo) · [截图](#截图) · [功能](#功能) · [已测试模型](#已测试模型) · [已测试平台](#已测试平台) · [安装](#安装) · [快速开始](#快速开始) · [文档](#docs)

</div>

---

## 截图

<p align="center">
  <img width="100%" alt="Devo 终端 TUI coding agent 在本地仓库中显示模型、上下文和 token 状态" src="./.github/assets/devo-terminal-tui-coding-agent-screenshot.png" />
</p>

<p align="center">
  <img width="100%" alt="Devo desktop coding agent app 展示仓库对话、项目侧边栏和模型控制" src="./.github/assets/devo-desktop-coding-agent-screenshot.png" />
</p>

## 为什么选择 Devo

Devo 面向那些不想绑定单一托管模型生态、需要掌控模型选择、运行时行为和
Desktop 体验、终端工作流以及工作区执行边界的团队。

- **接入任意模型** - 通过 provider/model 绑定接入 OpenAI 兼容 Chat
  Completions、OpenAI 兼容 Responses、Anthropic Messages、DeepSeek、
  Qwen、Kimi 或私有模型网关。
- **适合私有化和内网环境** - 以单一本地 Rust 二进制运行，支持离线安装路径，
  可以指向内部端点，不依赖托管式 agent 服务。
- **Desktop 与终端双入口** - 用 Desktop app 完成可视化上手和日常编码，也可以在需要
  终端原生自动化、远程 shell 或脚本化流程时使用 CLI/TUI。
- **面向 Agent Runtime 扩展** - MCP server、可复用 skills、可审计会话、权限控制
  和多 agent 流程都是运行时能力，不是一次性 prompt。

## 功能

- **模型中立的 provider runtime** - 通过 provider/model 绑定接入 OpenAI 兼容、
  Anthropic 兼容、DeepSeek、Qwen、Kimi、GLM、MiniMax、Xiaomi MiMo、
  OpenRouter 或本地端点。
- **MCP 支持** - 通过
  [Model Context Protocol](https://modelcontextprotocol.io/) 服务器连接外部工具和上下文。
  可用 CLI 管理：`devo mcp add|list|enable|disable|remove`（见
  [配置](./docs/configuration.zh-Hans.md#mcp-服务器)）。
- **Skill 支持** - 将可复用工作流、说明、脚本和参考资料打包成可复用的
  [Agent Skills](https://agentskills.io/)。
- **长任务支持** - 让 Devo 在多轮工作中自动管理上下文，避免任务变长后丢失上下文。
- **多代理支持** - 将工作拆分给专门代理，同时在会话中保持协调过程可见。
- **Plan Mode** - 在实现开始前，把较大的任务拆成清晰的多步骤计划。
- **并行工具调用** - 并行运行多个相互独立的工具，让模型少等待、多推进。
- **带权限的工具执行** - 在敏感工具调用触碰工作区前进行审查。
- **可审计会话** - 保留模型输出、工具调用、审批、token 用量和会话历史，
  方便检查和恢复。
- **成本和上下文可见性** - 在提供商支持时显示输入/输出 token、缓存 token
  和上下文窗口用量。
- **轻量级 Rust 运行时** - 使用 Rust 构建，内存开销低，本地运行时紧凑。
- **内置语义代码搜索（MCP）** - 可选的捆绑 MCP 服务器（`code_search` /
  `devo-code-search-mcp`），**默认关闭**。在本地 CPU 上运行代码嵌入模型，并结合
  密集检索与 BM25 关键词匹配，相比仅用 grep/find 的代理减少代码搜索上下文。用
  `devo mcp enable code_search` 或 TUI `/mcps` 启用。

## 已测试模型

<p>
  <img alt="DeepSeek v4 Flash / Pro" src="https://img.shields.io/badge/DeepSeek-v4%20Flash%20%2F%20Pro-4D6BFE?style=flat-square&logo=deepseek&logoColor=white" />
  <img alt="GLM 5.2" src="https://img.shields.io/badge/GLM-5.2-7856FF?style=flat-square&logo=huggingfacei&logoColor=white" />
  <img alt="MiniMax M3" src="https://img.shields.io/badge/MiniMax-M3-0B5FFF?style=flat-square&logo=minimax&logoColor=white" />
  <img alt="Qwen3 Coder Next" src="https://img.shields.io/badge/Qwen3-Coder%20Next-615CED?style=flat-square&logo=qwen&logoColor=white" />
  <img alt="Kimi K3" src="https://img.shields.io/badge/Kimi-K3-111111?style=flat-square&logo=moonshotai&logoColor=white" />
</p>

Devo 的内置模型目录包含 Qwen、Kimi、MiniMax、GLM 和 DeepSeek 的已测试模型定义。
Provider 端点仍可通过 provider/model 绑定配置。

## 已测试平台

<p>
  <img alt="macOS 已测试" src="https://img.shields.io/badge/macOS-tested-000000?style=flat-square&logo=apple&logoColor=white" />
  <img alt="Linux 已测试" src="https://img.shields.io/badge/Linux-tested-FCC624?style=flat-square&logo=linux&logoColor=000000" />
  <img alt="Windows 已测试" src="https://img.shields.io/badge/Windows-tested-0078D4?style=flat-square&logo=windows&logoColor=white" />
</p>

Devo 已在 macOS、Linux、Windows 和麒麟操作系统上测试。

### 面向中国企业用户

<p>
  <img alt="麒麟操作系统已测试" src="https://img.shields.io/badge/Kylin%20OS-tested-1E88E5?style=flat-square" />
  <img alt="HarmonyOS 支持已在路线图中" src="https://img.shields.io/badge/HarmonyOS-on%20the%20road-111111?style=flat-square&logo=harmonyos&logoColor=white" />
</p>

之所以单独标出麒麟操作系统覆盖，是因为在中国企业环境中，国产操作系统经常是实际部署要求的一部分。
HarmonyOS 支持已在路线图中；欢迎拥有 HarmonyOS 设备的贡献者为该平台构建、测试并发布版本。

## 安装

Devo 有两种安装形态。需要图形化 coding agent 工作区时选择 Desktop app；
偏好 shell-first 开发时选择终端原生的 TUI/CLI；也可以在同一台机器上同时安装两者。

### 选项一：Desktop App

如果你想使用图形化 Devo 体验，请从
[GitHub Releases](https://github.com/7df-lab/devo/releases/latest)
下载最新的 Devo Desktop 安装包，并选择与你的操作系统和架构匹配的 asset：

- **macOS** - 下载 `devo-desktop-...-mac-...` 的 `.dmg` 或 `.zip` asset。
- **Windows** - 下载 `devo-desktop-...-windows-...` 的 `.exe` asset。
- **Linux** - 下载 `devo-desktop-...-linux-...` 的 `.AppImage`、`.deb`
  或 `.rpm` asset。

**如果 macOS 显示以下错误，这是正常现象：“Devo”已损坏，无法打开。**当前
macOS Desktop builds 尚未签名，因此安装后需要执行下面的命令，macOS 才能启动应用：

```bash
sudo xattr -dr com.apple.quarantine /Applications/Devo.app
```

### 选项二：TUI / CLI

如果你更喜欢终端 TUI、需要 shell 自动化，或希望和 Desktop app 搭配使用，
请安装终端原生的 `devo` 命令。

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
```

Windows:

```powershell
irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

在线安装器会把 `devo` 放到 Devo home 目录下，安装用于快速仓库搜索的
`rg` sidecar，并支持可选安装 `code_search` 使用的本地模型。

<details>
<summary>可选：预安装本地 <code>code_search</code> 模型</summary>

仅在希望安装阶段就下载 Hugging Face 模型时使用。

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh -s -- --install-code-search-model
```

Windows:

```powershell
$env:DEVO_INSTALL_CODE_SEARCH_MODEL = "1"; irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

</details>

将现有安装升级到最新 release：

```bash
devo upgrade
```

升级命令会执行同一套平台安装器，安装器会打印版本变化，例如
`Version: v0.1.12 -> v0.1.15`。

如需在内网或无网络环境中安装，请参阅
[离线安装](./docs/offline-installation.zh-Hans.md)。

## 快速开始

配置 provider，打开一个仓库，然后启动 TUI：

```bash
cd /path/to/your/repo
devo onboard
```

常用命令：

```bash
devo                         # 在当前仓库启动交互式 TUI
devo resume <session-id>
```

## 配置

`devo onboard` 是推荐的设置路径。它会把 provider 和 model binding 写入
`config.toml`，并把 API key 保存在用户级 `auth.json`。

如需手动接入自有 API key 与自定义模型：

1. 在 `config.toml` 中定义 `[model.<slug>]` 参数、`[providers.<id>]` 和
   `[model_bindings.<id>]`。
2. 把密钥写入 `DEVO_HOME/auth.json`，并在 `[providers.<id>].credential` 中引用该
   credential id — 不要把 API key 本身写进 `config.toml`。
3. 将 `invocation_method` 设为与端点协议一致：`openai_chat_completions`、
   `openai_responses` 或 `anthropic_messages`。

完整示例（自定义模型参数 + API key）与协议说明见
[配置](./docs/configuration.zh-Hans.md#接入自有-api-key)。

## 响应较慢的 Provider

Provider 响应没有应用层总超时，因此本地模型可以按需要花费时间加载或生成输出。
Provider 连接建立仍有内部期限，并且用户可以随时取消请求。

## Docs

- [离线安装](./docs/offline-installation.zh-Hans.md)
- [配置](./docs/configuration.zh-Hans.md)

## 常见问题

### 项目状态如何？

Devo 仍处于 1.0 之前并在积极开发中。它已经适合本地评估、实验和贡献者使用；
公共 API 和配置仍可能变化。

### 支持哪些模型？

内置模型元数据目前覆盖 Qwen、Kimi、MiniMax、GLM 和 DeepSeek 系列。
任何支持 OpenAI 兼容 Chat Completions、OpenAI 兼容 Responses 或
Anthropic Messages API 的模型端点，都可以通过 provider/model 绑定接入。

### 如何接入自有 API key？

使用 `devo onboard`，或在 `config.toml` 中手动定义自定义 `[model.<slug>]`、
provider 与 binding，把 key 存入用户级 `auth.json`，并将 `invocation_method`
设为 `openai_chat_completions`、`openai_responses` 或 `anthropic_messages`。详见
[配置](./docs/configuration.zh-Hans.md#接入自有-api-key)。

### 应该使用 Desktop app 还是 TUI/CLI？

如果你需要可视化上手、会话浏览和图形化 coding workspace，请使用 Desktop app。
如果你需要终端原生自动化、远程 shell 工作流，或希望 coding agent 留在现有命令行环境中，
请使用 TUI/CLI。两种入口都面向同一个本地 Devo runtime。

## 参与贡献

项目仍处于早期阶段，欢迎贡献：

- 关于 client/server runtime、provider layer、safety model 和 TUI 的架构反馈。
- 文档和翻译。
- Provider、model 和 wire API 覆盖。
- 带验证命令和回归测试的聚焦修复。

请打开 issue 或 pull request 讨论变更。

## 许可证

本项目采用 [MIT License](./LICENSE) 授权。

---

**如果你觉得 Devo 有用，请考虑给它一个 star。**
