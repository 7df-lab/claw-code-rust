# 配置

[English](./configuration.md) | [简体中文](./configuration.zh-Hans.md) | [繁體中文](./configuration.zh-Hant.md) | [日本語](./configuration.ja.md) | [Русский](./configuration.ru.md)

`devo onboard` 是推薦的設定路徑。如需手動配置，Devo 會按以下順序合併設定：

1. 內建預設值
2. `DEVO_HOME/config.toml` - 使用者級配置，預設在 macOS/Linux 上為
   `~/.devo/config.toml`，在 Windows 上為 `C:\Users\yourname\.devo\config.toml`
3. `<workspace>/.devo/config.toml` - 專案級配置
4. CLI flags

憑據單獨保存在 `DEVO_HOME/auth.json`；`config.toml` 應引用 credential id，
而不是直接儲存 API key。

最小結構（內建模型 + provider 綁定）：

```toml
[defaults]
model_binding = "deepseek-v4-flash-api-deepseek-com"

[providers."api.deepseek.com"]
enabled = true
name = "api.deepseek.com"
base_url = "https://api.deepseek.com"
credential = "api_deepseek_com_api_key"
wire_apis = ["openai_chat_completions"]

[model_bindings.deepseek-v4-flash-api-deepseek-com]
enabled = true
model_slug = "deepseek-v4-flash"
provider = "api.deepseek.com"
request_model = "deepseek-v4-flash"
display_name = "DeepSeek V4 Flash"
invocation_method = "openai_chat_completions"
default_reasoning_effort = "high"
```

關鍵區分如下：

- `model_slug` 按 slug 選擇 Devo 的本地模型中繼資料。
- binding 的 `provider` 選擇一個 `[providers.<id>]` 連線記錄。
- `request_model` 是傳送到 provider 的模型 id。
- `invocation_method` 選擇實際使用的 provider 協議。詳見
  [呼叫方式（Invocation methods）](#呼叫方式invocation-methods)。

模型中繼資料也有 `provider` 欄位，它描述模型所需的 wire API；binding 的
`invocation_method` 則選擇執行階段連線，兩者應保持一致。API key 仍保存在
`auth.json` 中，並透過 provider 的 `credential` 參照連線。

沿用 `model_name` 的舊配置仍可讀取。下次儲存該 binding 時，Devo 會寫成
`request_model`。

## 接入自有 API key

Devo 不會把 API key 寫進 `config.toml`。接入自有 key 時：

1. 把密鑰保存在使用者級 `DEVO_HOME/auth.json`。
2. 在 `config.toml` 中透過 `[providers.<id>].credential` 引用該 credential id。

`devo onboard` 以及 Desktop/TUI 的 provider 流程會為你寫入這兩個檔案。

### 完整範例：自訂模型參數 + 自有 API key

下面同時設定自訂 DeepSeek 模型（Anthropic Messages）、provider 端點，以及只存放在
`auth.json` 中的憑據。

`~/.devo/config.toml`（Windows 為 `C:\Users\yourname\.devo\config.toml`）：

```toml
[defaults]
model_binding = "deepseek-example"

[model.my-deepseek]
display_name = "DeepSeek V4 Flash"
description = "Custom Anthropic Messages coding model for DeepSeek."
channel = "Custom"
# 該模型期望的 wire API，需與 binding 的 invocation_method 一致。
provider = "anthropic_messages"
context_window = 200000
effective_context_window_percent = 95
max_tokens = 8192
temperature = 0.2
reasoning_capability = { togglewithlevels = ["high", "max"] }
reasoning_implementation = "request_parameter"
base_instructions = "(optional) You are Devo, a coding agent."
input_modalities = ["text", "image"]

[providers.deepseek]
enabled = true
name = "DeepSeek Anthropic Compatible"
base_url = "https://api.deepseek.com/anthropic"
# 僅 credential id — 密鑰保存在 auth.json。
credential = "deepseek_compatible_api_key"
wire_apis = ["anthropic_messages"]

[model_bindings.deepseek-example]
enabled = true
model_slug = "my-deepseek"
provider = "deepseek"
request_model = "deepseek-v4-flash"
display_name = "DeepSeek V4 Flash"
invocation_method = "anthropic_messages"
```

對應的 `~/.devo/auth.json`（Windows 為 `C:\Users\yourname\.devo\auth.json`）：

```json
{
  "version": 1,
  "credentials": {
    "deepseek_compatible_api_key": {
      "kind": "api_key",
      "value": "sk-deepseek-your-api-key"
    }
  }
}
```

規則：

- 目前只支援 `api_key` 類型憑據。
- credential id 必須與 `[providers.<id>].credential` 完全一致。
- 將 `auth.json` 保留在 `DEVO_HOME` 下，不要提交到專案倉庫。
- 工作區 `<workspace>/.devo/config.toml` 可以引用 credential id，但密鑰值只存在
  使用者級 `auth.json`。
- 僅更新 key 時編輯 `auth.json`；credential id 不變時無需改 `config.toml`。

## 呼叫方式（Invocation methods）

binding 上的 `invocation_method` 與 provider 上的 `wire_apis` 決定 Devo 使用哪種
HTTP API。模型中繼資料裡的 `provider` 應使用相同值，以便目錄能力與執行階段連線一致。

| 取值 | 協議 | 典型端點 |
| --- | --- | --- |
| `openai_chat_completions` | [OpenAI Chat Completions](https://developers.openai.com/api/reference/chat-completions/overview) | 多數 OpenAI 相容閘道（DeepSeek、Qwen、Kimi、OpenRouter、許多本地代理） |
| `openai_responses` | [OpenAI Responses](https://developers.openai.com/api/reference/responses/overview) | 提供 Responses API 的服務 |
| `anthropic_messages` | [Anthropic Messages](https://platform.claude.com/docs/en/api/messages) | Anthropic 相容 Messages 端點 |

## 模型中繼資料與自訂模型

在使用者或工作區 `config.toml` 的 `[model.<slug>]` 下設定模型中繼資料。內建 slug
使用部分覆蓋，未寫欄位保留內建值；新 slug 會建立帶安全預設值的自訂模型，
並應透過 `[providers.<id>]` 和 `[model_bindings.<id>]` 連線，參見
[完整範例](#完整範例自訂模型參數--自有-api-key)。

內建模型部分覆蓋範例：

```toml
[model.qwen3-coder-next]
context_window = 262144
effective_context_window_percent = 90
```

有效上下文視窗的精確公式是
`context_window * effective_context_window_percent / 100`；結果既是模型可用上下文，
也是自動壓縮邊界。

可設定中繼資料包括：`display_name`（選擇器中的名稱）、`description`（說明文字）、
`channel`（分組標籤）。`context_window` 與
`effective_context_window_percent` 決定有效上下文，`max_tokens` 是預設輸出上限。
取樣預設值：`temperature`（隨機性）、`top_p`（核取樣）、`top_k`（候選 token 上限）。
`provider` wire API 取值為 `openai_chat_completions`、`openai_responses` 或
`anthropic_messages`。推理中繼資料是類型化的：`reasoning_capability` 可為
`unsupported`、`toggle`、`{ levels = [...] }` 或 `{ togglewithlevels = [...] }`；
`reasoning_implementation` 可為 `disabled`、`request_parameter` 或類型化的
`model_variant` 表。`model_variant` 把邏輯推理選擇對應到不同的 provider 模型 id、
可選有效 effort，以及可選額外請求體，而不是在同一模型上改參數；
`default_reasoning_effort` 選擇預設 effort。`input_modalities` 接受 `text` 和
`image`；`truncation_policy` 為過大的工具結果選擇位元組或 token 上限；
`supports_image_detail_original` 啟用原始影像細節。

省略 `base_instructions` 時，內建模型保留內建指令，自訂模型使用 Devo 預設指令。
明確空字串（`base_instructions = ""`）表示無基礎指令。

舊版純量 `model = "slug"` 仍可讀取。因 `[model.<slug>]` 佔用了頂層 `model`
表命名空間，新配置須用 `[defaults].model_binding` 選擇活躍連線。

### TUI 偏好

`DEVO_HOME/config.toml` 頂層還儲存部分 UI 偏好：

```toml
theme = "aurora"
collapse_reasoning = true
compaction_token_limit = 250000
```

- `theme` 選擇 TUI 配色主題（也可透過 Settings › Appearance 設定）。
- `collapse_reasoning` 控制推理顯示（也可透過 `/show-reasoning` 設定）：
  - `true`（預設）：串流輸出時只顯示最新 3 行；結束後短推理完整保留，較長推理摺疊為
    一行 `Thought · …` 摘要（完整文字仍可在 Ctrl+T 檢視）。
  - `false`：串流輸出與結束後都顯示完整推理。
- `compaction_token_limit` 為全域自動壓縮絕對 token 閾值（也可透過 Settings ›
  Compaction threshold 設定）。設定後，每個 session 會將此值 clamp 到當前模型的
  `context_window`；未設定時沿用模型有效上下文窗口。

### 從 `models.json` 遷移

舊的 `~/.devo/models.json` 與 `<workspace>/.devo/models.json` 會被忽略。
請手動把仍需使用的欄位複製到使用者或工作區 `config.toml` 的 `[model.<slug>]`
段，並新增或保留對應 provider 和 model binding。API key 繼續放在 `auth.json`，
透過 `[providers.<id>].credential` 引用。

## MCP 伺服器

Devo 透過使用者或工作區 `config.toml` 中的 `[mcp]` 設定
[Model Context Protocol](https://modelcontextprotocol.io/) 伺服器。每個伺服器是
`servers` 陣列中的一項，其 `transport` 表決定 Devo 的連線方式。支援的傳輸方式有
`stdio`、`streamable_http` 和已棄用的 `sse`。

可用編輯 `config.toml` 或 CLI（`devo mcp …`）設定 MCP。日常新增 / 啟用 / 停用 /
刪除優先用 CLI；需要細調傳輸參數、環境變數或 header 時再改 TOML。

### 捆綁的 `code_search`（預設關閉）

Devo 會在 `devo` 旁邊安裝可選的語義搜尋 MCP 二進位。設定項在缺失時會自動注入，
且保持 **disabled**，直到你明確啟用：

```toml
[[mcp.servers]]
id = "code_search"
display_name = "Code Search"
enabled = false
startup_policy = "lazy"

[mcp.servers.transport]
kind = "stdio"
command = ["devo-code-search-mcp"]
```

```bash
devo mcp enable code_search
# 或在互動工作階段：/mcps → Code Search → Enable
```

啟用後，模型側工具名稱為 `mcp__code_search__code_search`。

### CLI 管理

用 `devo mcp` 管理使用者級 MCP 伺服器（`~/.devo/config.toml`）：

```bash
devo mcp list
devo mcp add time -- docker run -i --rm mcp/time
devo mcp add filesystem --env HOME=/tmp -- npx -y @modelcontextprotocol/server-filesystem .
devo mcp add --transport http hello-mcp http://localhost:8080/mcp
devo mcp add --transport http github --bearer-token "$TOKEN" https://api.githubcopilot.com/mcp/
devo mcp add --transport sse legacy-mcp https://example.com/mcp/sse
devo mcp enable time
devo mcp disable time
devo mcp remove time
```

CLI `enable|disable` 會寫入使用者 `config.toml`。已在執行的互動工作階段透過
TUI `/mcps`（`mcp/set_enabled`）即時套用。

### TOML 範例

stdio 範例：

```toml
[mcp]
auto_start = true

[[mcp.servers]]
id = "filesystem"
display_name = "Filesystem"
enabled = true
startup_policy = "lazy" # eager | lazy | manual
trust_policy = "user" # user | workspace | untrusted
allowed_capabilities = ["tools", "resources", "prompts"]
roots_policy = "workspace" # none | workspace | custom

[mcp.servers.transport]
kind = "stdio"
command = ["npx", "-y", "@modelcontextprotocol/server-filesystem", "."]
# cwd = "/path/to/workdir"
# env = { MY_VAR = "value" }
# env_vars = ["HOME", "PATH"]
```

帶 bearer token 的 Streamable HTTP：

```toml
[[mcp.servers]]
id = "github"
display_name = "GitHub"
startup_policy = "lazy"

[mcp.servers.transport]
kind = "streamable_http"
url = "https://api.githubcopilot.com/mcp/"
auth = { kind = "bearer_token", token = "replace-me" }
http_headers = { "X-Custom" = "static-value" }
env_http_headers = { "Authorization" = "GITHUB_TOKEN" }
```

舊版 SSE 傳輸：

```toml
[mcp.servers.transport]
kind = "sse"
url = "https://example.com/mcp/sse"
```

欄位說明：

- `auto_start` 預設為 `true`。執行中工作階段的 MCP 啟用/停用會透過 `mcp/set_enabled`（TUI `/mcps`）即時套用。
- `startup_policy` 控制已啟用伺服器的啟動時機：`eager` 在啟動階段啟動，`lazy`
  首次使用時啟動，`manual` 僅依明確請求啟動。
- stdio 下，`env` 提供字面值，`env_vars` 列出從本機環境繼承的變數名稱；
  stdio 不支援 `{ name = "X", source = "remote" }`。
- HTTP 傳輸下，`http_headers` 提供字面 header，`env_http_headers` 將 header
  名稱對應到提供其值的環境變數名稱。
- `allowed_capabilities` 為空表示不限制。目前執行階段主要接入 `tools`，資源讀取
  尚未接線。
- `output_limits` 設定 `max_tool_output_bytes`（預設 1 MiB）與
  `max_resource_bytes`（預設 10 MiB）。
- 頂層 `mcp_oauth_credentials_store` 取值為 `auto`（預設）、`file` 或
  `keyring`，選擇 OAuth 憑據的儲存位置。
- 盡量用環境變數注入 header 或值，避免把 token 硬編碼進 `config.toml`。
  `auth_ref` 欄位已存在於每個伺服器記錄中，但尚未接入執行階段。

合併行為：`[mcp]` 與其他表一樣依欄位合併，但 `servers` 是陣列。專案級的
`[[mcp.servers]]` 列表會整體取代使用者級列表，而不是依 `id` 合併。

可在 TUI 中用 `/mcps`（互動式清單 → 詳情 → 工具）驗證配置。客戶端可呼叫
`mcp/list`、`mcp/tools`、`mcp/set_enabled` RPC。
