<div align="center">

<img src="./.github/assets/devo-readme-brand.svg" alt="Devo desktop coding agent app icon and wordmark" width="360" />

</div>

<div align="center">

**Devo 是開源 coding agent，提供 Desktop app、終端機 TUI/CLI 和模型中立的 Rust runtime，面向私有化、企業內網和 OpenAI 相容模型環境。可接入 DeepSeek、Qwen、Kimi、Anthropic 相容 API、本地閘道或自訂模型端點。**

[![Stars](https://img.shields.io/github/stars/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/stargazers)
[![Language](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/7df-lab/devo/pulls)
[![CI](https://img.shields.io/github/actions/workflow/status/7df-lab/devo/ci.yml?branch=main&style=flat-square)](https://github.com/7df-lab/devo/actions)
[![Release](https://img.shields.io/github/v/release/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/releases)

[English](./README.md) | [简体中文](./README.zh-Hans.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | [Русский](./README.ru.md)

[為什麼選擇 Devo](#為什麼選擇-devo) · [截圖](#截圖) · [功能](#功能) · [已測試模型](#已測試模型) · [已測試平台](#已測試平台) · [安裝](#安裝) · [快速開始](#快速開始) · [文件](#docs)

</div>

---

## 截圖

<p align="center">
  <img width="100%" alt="Devo desktop coding agent app 展示儲存庫對話、專案側邊欄和模型控制" src="./.github/assets/devo-desktop-coding-agent-screenshot.png" />
</p>

<p align="center">
  <img width="100%" alt="Devo 終端機 TUI coding agent 在本地儲存庫中顯示模型、上下文和 token 狀態" src="./.github/assets/devo-terminal-tui-coding-agent-screenshot.png" />
</p>

## 為什麼選擇 Devo

Devo 面向那些不想綁定單一託管模型生態、需要掌控模型選擇、執行階段行為和
Desktop 體驗、終端機工作流以及工作區執行邊界的團隊。

- **接入任意模型** - 透過 provider/model 綁定接入 OpenAI 相容 Chat
  Completions、OpenAI 相容 Responses、Anthropic Messages、DeepSeek、
  Qwen、Kimi 或私有模型閘道。
- **適合私有化和內網環境** - 以單一本地 Rust 二進位檔執行，支援離線安裝路徑，
  可以指向內部端點，不依賴託管式 agent 服務。
- **Desktop 與終端機雙入口** - 用 Desktop app 完成可視化上手和日常編碼，也可以在需要
  終端機原生自動化、遠端 shell 或腳本化流程時使用 CLI/TUI。
- **面向 Agent Runtime 擴展** - MCP server、可重用 skills、可稽核會話、權限控制
  和多 agent 流程都是執行階段能力，不是一次性 prompt。

## 功能

- **模型中立的 provider runtime** - 透過 provider/model 綁定接入 OpenAI 相容、
  Anthropic 相容、DeepSeek、Qwen、Kimi、GLM、MiniMax、Xiaomi MiMo、
  OpenRouter 或本地端點。
- **MCP 支援** - 透過
  [Model Context Protocol](https://modelcontextprotocol.io/) 伺服器連接外部工具和上下文。
  可用 CLI 管理：`devo mcp add|list|enable|disable|remove`（見
  [配置](./docs/configuration.zh-Hant.md#mcp-伺服器)）。
- **Skill 支援** - 將可重複工作流程、說明、腳本和參考資料打包成可重用的
  [Agent Skills](https://agentskills.io/)。
- **長時間任務支援** - 讓 Devo 在多輪工作中自動管理上下文，避免任務變長後丟失脈絡。
- **多代理支援** - 將工作拆分給專門代理，同時在會話中保持協調過程可見。
- **Plan Mode** - 在實作開始前，把較大的任務拆成清晰的多步驟計畫。
- **平行工具呼叫** - 平行執行多個相互獨立的工具，讓模型少等待、多推進。
- **帶權限的工具執行** - 在敏感工具呼叫觸碰工作區前進行審查。
- **可稽核會話** - 保留模型輸出、工具呼叫、審批、token 用量和會話歷史，
  方便檢查和恢復。
- **成本和上下文可見性** - 在供應商支援時顯示輸入/輸出 token、快取 token
  和上下文視窗用量。
- **輕量級 Rust 執行階段** - 使用 Rust 建構，記憶體開銷低，本地執行階段緊湊。
- **內建語義程式碼搜尋（MCP）** - 可選的捆綁 MCP 伺服器（`code_search` /
  `devo-code-search-mcp`），**預設關閉**。在本地 CPU 執行程式碼嵌入模型，並結合
  密集檢索與 BM25 關鍵字比對，相比僅用 grep/find 的代理減少程式碼搜尋上下文。用
  `devo mcp enable code_search` 或 TUI `/mcps` 啟用。

## 已測試模型

<p>
  <img alt="DeepSeek v4 Flash / Pro" src="https://img.shields.io/badge/DeepSeek-v4%20Flash%20%2F%20Pro-4D6BFE?style=flat-square&logo=deepseek&logoColor=white" />
  <img alt="GLM 5.2" src="https://img.shields.io/badge/GLM-5.2-7856FF?style=flat-square&logo=huggingfacei&logoColor=white" />
  <img alt="MiniMax M3" src="https://img.shields.io/badge/MiniMax-M3-0B5FFF?style=flat-square&logo=minimax&logoColor=white" />
  <img alt="Qwen3 Coder Next" src="https://img.shields.io/badge/Qwen3-Coder%20Next-615CED?style=flat-square&logo=qwen&logoColor=white" />
  <img alt="Kimi K3" src="https://img.shields.io/badge/Kimi-K3-111111?style=flat-square&logo=moonshotai&logoColor=white" />
</p>

Devo 的內建模型目錄包含 Qwen、Kimi、MiniMax、GLM 和 DeepSeek 的已測試模型定義。
Provider 端點仍可透過 provider/model 綁定配置。

## 已測試平台

<p>
  <img alt="macOS 已測試" src="https://img.shields.io/badge/macOS-tested-000000?style=flat-square&logo=apple&logoColor=white" />
  <img alt="Linux 已測試" src="https://img.shields.io/badge/Linux-tested-FCC624?style=flat-square&logo=linux&logoColor=000000" />
  <img alt="Windows 已測試" src="https://img.shields.io/badge/Windows-tested-0078D4?style=flat-square&logo=windows&logoColor=white" />
</p>

Devo 已在 macOS、Linux、Windows 和麒麟作業系統上測試。

### 面向中國企業使用者

<p>
  <img alt="麒麟作業系統已測試" src="https://img.shields.io/badge/Kylin%20OS-tested-1E88E5?style=flat-square" />
  <img alt="HarmonyOS 支援已在路線圖中" src="https://img.shields.io/badge/HarmonyOS-on%20the%20road-111111?style=flat-square&logo=harmonyos&logoColor=white" />
</p>

之所以單獨標出麒麟作業系統覆蓋，是因為在中國企業環境中，國產作業系統經常是實際部署要求的一部分。
HarmonyOS 支援已在路線圖中；歡迎擁有 HarmonyOS 裝置的貢獻者為該平台建構、測試並發布版本。

## 安裝

Devo 有兩種安裝形態。需要圖形化 coding agent 工作區時選擇 Desktop app；
偏好 shell-first 開發時選擇終端機原生的 TUI/CLI；也可以在同一台機器上同時安裝兩者。

### 選項一：Desktop App

如果你想使用圖形化 Devo 體驗，請從
[GitHub Releases](https://github.com/7df-lab/devo/releases/latest)
下載最新的 Devo Desktop 安裝包，並選擇與你的作業系統和架構匹配的 asset：

- **macOS** - 下載 `devo-desktop-...-mac-...` 的 `.dmg` 或 `.zip` asset。
- **Windows** - 下載 `devo-desktop-...-windows-...` 的 `.exe` asset。
- **Linux** - 下載 `devo-desktop-...-linux-...` 的 `.AppImage`、`.deb`
  或 `.rpm` asset。

**如果 macOS 提示「Devo」已損壞，無法打開，這是正常現象。**目前 macOS
Desktop builds 尚未簽名，因此安裝後需要執行下面的命令，macOS 才能啟動應用：

```bash
sudo xattr -dr com.apple.quarantine /Applications/Devo.app
```

### 選項二：TUI / CLI

如果你更喜歡終端機 TUI、需要 shell 自動化，或希望和 Desktop app 搭配使用，
請安裝終端機原生的 `devo` 命令。

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
```

Windows:

```powershell
irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

線上安裝器會把 `devo` 放到 Devo home 目錄下，安裝用於快速儲存庫搜尋的
`rg` sidecar，並支援可選安裝 `code_search` 使用的本地模型。

<details>
<summary>可選：預安裝本地 <code>code_search</code> 模型</summary>

僅在希望安裝階段就下載 Hugging Face 模型時使用。

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh -s -- --install-code-search-model
```

Windows:

```powershell
$env:DEVO_INSTALL_CODE_SEARCH_MODEL = "1"; irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

</details>

將現有安裝升級到最新 release：

```bash
devo upgrade
```

升級命令會執行同一套平台安裝器，安裝器會列印版本變化，例如
`Version: v0.1.12 -> v0.1.15`。

如需在內網或無網路環境中安裝，請參閱
[離線安裝](./docs/offline-installation.zh-Hant.md)。

## 快速開始

配置 provider，開啟一個儲存庫，然後啟動 TUI：

```bash
cd /path/to/your/repo
devo onboard
```

常用命令：

```bash
devo                         # 在目前儲存庫啟動互動式 TUI
devo resume <session-id>
```

## 配置

`devo onboard` 是推薦的設定路徑。它會把 provider 和 model binding 寫入
`config.toml`，並把 API key 保存在使用者級 `auth.json`。

如需手動接入自有 API key 與自訂模型：

1. 在 `config.toml` 中定義 `[model.<slug>]` 參數、`[providers.<id>]` 和
   `[model_bindings.<id>]`。
2. 把密鑰寫入 `DEVO_HOME/auth.json`，並在 `[providers.<id>].credential` 中引用該
   credential id — 不要把 API key 本身寫進 `config.toml`。
3. 將 `invocation_method` 設為與端點協議一致：`openai_chat_completions`、
   `openai_responses` 或 `anthropic_messages`。

完整範例（自訂模型參數 + API key）與協議說明見
[配置](./docs/configuration.zh-Hant.md#接入自有-api-key)。

## Docs

- [離線安裝](./docs/offline-installation.zh-Hant.md)
- [配置](./docs/configuration.zh-Hant.md)

## 常見問題

### 專案狀態如何？

Devo 仍處於 1.0 之前並在積極開發中。它已經適合本地評估、實驗和貢獻者使用；
公共 API 和配置仍可能變更。

### 支援哪些模型？

內建模型中繼資料目前覆蓋 Qwen、Kimi、MiniMax、GLM 和 DeepSeek 系列。
任何支援 OpenAI 相容 Chat Completions、OpenAI 相容 Responses 或
Anthropic Messages API 的模型端點，都可以透過 provider/model 綁定接入。

### 如何接入自有 API key？

使用 `devo onboard`，或在 `config.toml` 中手動定義自訂 `[model.<slug>]`、
provider 與 binding，把 key 存入使用者級 `auth.json`，並將 `invocation_method`
設為 `openai_chat_completions`、`openai_responses` 或 `anthropic_messages`。詳見
[配置](./docs/configuration.zh-Hant.md#接入自有-api-key)。

### 應該使用 Desktop app 還是 TUI/CLI？

如果你需要可視化上手、會話瀏覽和圖形化 coding workspace，請使用 Desktop app。
如果你需要終端機原生自動化、遠端 shell 工作流，或希望 coding agent 留在現有命令列環境中，
請使用 TUI/CLI。兩種入口都面向同一個本地 Devo runtime。

## 參與貢獻

專案仍處於早期階段，歡迎貢獻：

- 關於 client/server runtime、provider layer、safety model 和 TUI 的架構回饋。
- 文件和翻譯。
- Provider、model 和 wire API 覆蓋。
- 帶驗證命令和迴歸測試的聚焦修復。

請開啟 issue 或 pull request 討論變更。

## 授權

本專案採用 [MIT License](./LICENSE) 授權。

---

**如果你覺得 Devo 有用，請考慮給它一個 star。**
