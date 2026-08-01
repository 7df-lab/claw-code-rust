# 配置

[English](./configuration.md) | [简体中文](./configuration.zh-Hans.md) | [繁體中文](./configuration.zh-Hant.md) | [日本語](./configuration.ja.md) | [Русский](./configuration.ru.md)

`devo onboard` 是推荐的设置路径。如需手动配置，Devo 会按以下顺序合并设置：

1. 内置默认值
2. `DEVO_HOME/config.toml` - 用户级配置，默认在 macOS/Linux 上为
   `~/.devo/config.toml`，在 Windows 上为 `C:\Users\yourname\.devo\config.toml`
3. `<workspace>/.devo/config.toml` - 项目级配置
4. CLI flags

凭据单独保存在 `DEVO_HOME/auth.json`；`config.toml` 应引用 credential id，
而不是直接存储 API key。

最小结构（内置模型 + provider 绑定）：

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

关键区分如下：

- `model_slug` 按 slug 选择 Devo 的本地模型元数据。
- binding 的 `provider` 选择一个 `[providers.<id>]` 连接记录。
- `request_model` 是发送到 provider 的模型 id。
- `invocation_method` 选择实际使用的 provider 协议。详见
  [调用方式（Invocation methods）](#调用方式invocation-methods)。

模型元数据也有 `provider` 字段，它描述模型所需的 wire API；binding 的
`invocation_method` 则选择运行时连接，二者应保持一致。API key 仍保存在
`auth.json` 中，并通过 provider 的 `credential` 引用连接。

沿用 `model_name` 的旧配置仍可读取。下次保存该 binding 时，Devo 会写成
`request_model`。

## 接入自有 API key

Devo 不会把 API key 写进 `config.toml`。接入自有 key 时：

1. 把密钥保存在用户级 `DEVO_HOME/auth.json`。
2. 在 `config.toml` 中通过 `[providers.<id>].credential` 引用该 credential id。

`devo onboard` 以及 Desktop/TUI 的 provider 流程会为你写入这两个文件。

### 完整示例：自定义模型参数 + 自有 API key

下面同时配置自定义 DeepSeek 模型（Anthropic Messages）、provider 端点，以及只存放在
`auth.json` 中的凭据。

`~/.devo/config.toml`（Windows 为 `C:\Users\yourname\.devo\config.toml`）：

```toml
[defaults]
model_binding = "deepseek-example"

[model.my-deepseek]
display_name = "DeepSeek V4 Flash"
description = "Custom Anthropic Messages coding model for DeepSeek."
channel = "Custom"
# 该模型期望的 wire API，需与 binding 的 invocation_method 一致。
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
# 仅 credential id — 密钥保存在 auth.json。
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

对应的 `~/.devo/auth.json`（Windows 为 `C:\Users\yourname\.devo\auth.json`）：

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

规则：

- 目前只支持 `api_key` 类型凭据。
- credential id 必须与 `[providers.<id>].credential` 完全一致。
- 将 `auth.json` 保留在 `DEVO_HOME` 下，不要提交到项目仓库。
- 工作区 `<workspace>/.devo/config.toml` 可以引用 credential id，但密钥值只存在
  用户级 `auth.json`。
- 仅更新 key 时编辑 `auth.json`；credential id 不变时无需改 `config.toml`。

## 调用方式（Invocation methods）

binding 上的 `invocation_method` 与 provider 上的 `wire_apis` 决定 Devo 使用哪种
HTTP API。模型元数据里的 `provider` 应使用相同值，以便目录能力与运行时连接一致。

| 取值 | 协议 | 典型端点 |
| --- | --- | --- |
| `openai_chat_completions` | [OpenAI Chat Completions](https://developers.openai.com/api/reference/chat-completions/overview) | 多数 OpenAI 兼容网关（DeepSeek、Qwen、Kimi、OpenRouter、许多本地代理） |
| `openai_responses` | [OpenAI Responses](https://developers.openai.com/api/reference/responses/overview) | 提供 Responses API 的服务 |
| `anthropic_messages` | [Anthropic Messages](https://platform.claude.com/docs/en/api/messages) | Anthropic 兼容 Messages 端点 |

## 模型元数据与自定义模型

在用户或工作区 `config.toml` 的 `[model.<slug>]` 下配置模型元数据。内置 slug
使用部分覆盖，未写字段保留内置值；新 slug 会创建带安全默认值的自定义模型，
并应通过 `[providers.<id>]` 和 `[model_bindings.<id>]` 连接，参见
[完整示例](#完整示例自定义模型参数--自有-api-key)。

内置模型部分覆盖示例：

```toml
[model.qwen3-coder-next]
context_window = 262144
effective_context_window_percent = 90
```

有效上下文窗口的精确公式是
`context_window * effective_context_window_percent / 100`；结果既是模型可用上下文，
也是自动压缩边界。

可配置元数据包括：`display_name`（选择器中的名称）、`description`（说明文字）、
`channel`（分组标签）。`context_window` 与
`effective_context_window_percent` 决定有效上下文，`max_tokens` 是默认输出上限。
采样默认值：`temperature`（随机性）、`top_p`（核采样）、`top_k`（候选 token 上限）。
`provider` wire API 取值为 `openai_chat_completions`、`openai_responses` 或
`anthropic_messages`。推理元数据是类型化的：`reasoning_capability` 可为
`unsupported`、`toggle`、`{ levels = [...] }` 或 `{ togglewithlevels = [...] }`；
`reasoning_implementation` 可为 `disabled`、`request_parameter` 或类型化的
`model_variant` 表。`model_variant` 把逻辑推理选择映射到不同的 provider 模型 id、
可选有效 effort，以及可选额外请求体，而不是在同一模型上改参数；
`default_reasoning_effort` 选择默认 effort。`input_modalities` 接受 `text` 和
`image`；`truncation_policy` 为过大的工具结果选择字节或 token 上限；
`supports_image_detail_original` 启用原始图像细节。

省略 `base_instructions` 时，内置模型保留内置指令，自定义模型使用 Devo 默认指令。
显式空字符串（`base_instructions = ""`）表示无基础指令。

旧版标量 `model = "slug"` 仍可读取。因 `[model.<slug>]` 占用了顶层 `model`
表命名空间，新配置须用 `[defaults].model_binding` 选择活跃连接。

### TUI 偏好

`DEVO_HOME/config.toml` 顶层还保存部分 UI 偏好：

```toml
theme = "aurora"
collapse_reasoning = true
```

- `theme` 选择 TUI 配色主题（也可通过 `/theme` 设置）。
- `collapse_reasoning` 控制推理显示（也可通过 `/show-reasoning` 设置）：
  - `true`（默认）：流式输出时只显示最新 3 行；结束后短推理完整保留，较长推理折叠为
    一行 `Thought · …` 摘要（完整文本仍可在 Ctrl+T 查看）。
  - `false`：流式输出与结束后都显示完整推理。

### 从 `models.json` 迁移

旧的 `~/.devo/models.json` 与 `<workspace>/.devo/models.json` 会被忽略。
请手动把仍需使用的字段复制到用户或工作区 `config.toml` 的 `[model.<slug>]`
段，并添加或保留对应 provider 和 model binding。API key 继续放在 `auth.json`，
通过 `[providers.<id>].credential` 引用。

## MCP 服务器

Devo 通过用户或工作区 `config.toml` 中的 `[mcp]` 配置
[Model Context Protocol](https://modelcontextprotocol.io/) 服务器。每个服务器是
`servers` 数组中的一项，其 `transport` 表决定 Devo 的连接方式。支持的传输方式有
`stdio`、`streamable_http` 和已弃用的 `sse`。

stdio 示例：

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

带 bearer token 的 Streamable HTTP：

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

旧版 SSE 传输：

```toml
[mcp.servers.transport]
kind = "sse"
url = "https://example.com/mcp/sse"
```

字段说明：

- `auto_start` 默认为 `true`。运行中会话的 MCP 启用/禁用通过 `mcp/set_enabled`（TUI `/mcps`）即时生效。
- `startup_policy` 控制已启用服务器的启动时机：`eager` 在启动阶段启动，`lazy`
  首次使用时启动，`manual` 仅按显式请求启动。
- stdio 下，`env` 提供字面量值，`env_vars` 列出从本地环境继承的变量名；
  stdio 不支持 `{ name = "X", source = "remote" }`。
- HTTP 传输下，`http_headers` 提供字面量 header，`env_http_headers` 将 header
  名映射到提供其值的环境变量名。
- `allowed_capabilities` 为空表示不限制。当前运行时主要接入 `tools`，资源读取
  尚未接线。
- `output_limits` 设置 `max_tool_output_bytes`（默认 1 MiB）与
  `max_resource_bytes`（默认 10 MiB）。
- 顶层 `mcp_oauth_credentials_store` 取值为 `auto`（默认）、`file` 或
  `keyring`，选择 OAuth 凭据的存储位置。
- 尽量用环境变量注入 header 或值，避免把 token 硬编码进 `config.toml`。
  `auth_ref` 字段已存在于每个服务器记录中，但尚未接入运行时。

合并行为：`[mcp]` 与其他表一样按字段合并，但 `servers` 是数组。项目级的
`[[mcp.servers]]` 列表会整体替换用户级列表，而不是按 `id` 合并。

### CLI 管理

用 `devo mcp` 管理用户级 MCP 服务器（`~/.devo/config.toml`）：

```bash
# Stdio（`--` 后为 command + args）
devo mcp add time -- docker run -i --rm mcp/time

# Streamable HTTP（`--transport http` 写入 kind = "streamable_http"）
devo mcp add --transport http hello-mcp http://localhost:8080/mcp

# 旧版 SSE
devo mcp add --transport sse legacy-mcp https://example.com/mcp/sse

devo mcp list
devo mcp enable time
devo mcp disable time
devo mcp remove time
```

CLI `devo mcp enable|disable` 会写入用户 `config.toml`（离线配置）。已在运行的
交互会话通过 TUI `/mcps`（`mcp/set_enabled` RPC）即时启用/禁用。

可在 TUI 中用 `/mcps`（交互式列表 → 详情 → 工具；Enable/Disable 会持久化配置并为
下一回合应用管理器与工具注册表）验证配置。客户端也可调用 `mcp/list`、
`mcp/tools`、`mcp/set_enabled`。也可用 `devo mcp add|list|remove|enable|disable`
管理用户级配置。
