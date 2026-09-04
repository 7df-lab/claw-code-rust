# 配置

[English](./configuration.md) | [简体中文](./configuration.zh-Hans.md) | [繁體中文](./configuration.zh-Hant.md) | [日本語](./configuration.ja.md) | [Русский](./configuration.ru.md)

`devo onboard` 是推荐的设置路径。如需手动配置，Devo 会按以下顺序合并设置：

1. 内置默认值
2. `DEVO_HOME/config.toml` - 用户级应用配置，默认在 macOS/Linux 上为
   `~/.devo/config.toml`，在 Windows 上为 `C:\Users\yourname\.devo\config.toml`
3. `DEVO_HOME/providers.json` - 用户级 provider 连接与模型选择
4. `<workspace>/.devo/config.toml` - 项目级应用配置
5. `<workspace>/.devo/providers.json` - 项目级 provider/model 覆盖
6. CLI flags

Provider API key 保存在用户级 `auth.json` 中。`providers.json` 只保存指向
密钥的 credential id；目录文件本身可以安全地跟踪和共享。

最小结构（内置目录模型 + provider 连接）：

```json
{
  "model": "deepseek/deepseek-v4-flash",
  "provider": {
    "deepseek": {
      "name": "DeepSeek",
      "base_url": "https://api.deepseek.com/anthropic",
      "credential": "deepseek_api_key",
      "wire_api": "anthropic_messages",
      "models": {
        "deepseek-v4-flash": {"name": "DeepSeek V4 Flash"}
      }
    }
  }
}
```

稳定的模型身份始终是 `provider/model`：provider map 的键是 provider id，
内部 models map 的键是发送给 provider 的模型 id。不再需要单独维护 binding id、
local slug、request model 或 model description。provider 和模型元数据都可以省略，
git 跟踪的目录提供默认值，用户/工作区文件只需添加或覆盖必要字段。

git 跟踪的内置目录是 `crates/core/providers.json`，会随 Devo 一起打包。
`DEVO_HOME/providers.json` 与 `<workspace>/.devo/providers.json` 是覆盖层，
可以定义任意自定义 provider 和模型。旧 TOML provider 结构仍可读取用于迁移，
但 onboarding 和 provider/model 新写入都会使用 `providers.json`。

顶层 `model` 是普通 turn 使用的默认主模型，格式为 `provider/model`。
可选的 `small_model` 用于标题生成等轻量后台任务。未设置时，会先在主模型
所属 provider 中自动寻找明显的轻量模型（例如 `flash`、`nano`、`haiku`、
`mini`、`small` 或小参数量模型），找不到才回退到主模型。显式配置的
`small_model` 如果无效，也会走同样的自动回退路径。git 跟踪的内置目录不预设
这两个选择，避免替用户选定 provider。

当前目录已经内置官方条目：Kimi（`kimi-k3`、`kimi-k2.7-code`、
`kimi-k2.6`）、Z.ai 和智谱 BigModel（各自只保留 `glm-5.3`、
`glm-5.3-flash`）、
DeepSeek、通义千问、MiniMax、
小米 MiMo 和腾讯混元。DeepSeek 默认使用官方 Anthropic 兼容端点。
同时内置本地 `ollama` provider 模板（默认 `http://localhost:11434/v1`），
模型目录为空；连接后通过 Discover（Ollama `/api/tags` 或 OpenAI 兼容的
`/v1/models`）拉取本机已安装的模型。

这个目录是精选的起点，不是封闭的白名单。用户或工作区只需在
`providers.json` 覆盖层中按相同的嵌套结构增加任意 provider 和模型即可。

### Provider 模板与 Connection

目录中的 provider 是只读模板，不是用户已经登录的 provider。内置 provider
只提供名称、默认 Base URL、协议和模型目录。用户在 onboarding 中确认后，
才会在用户级 `providers.json` 中创建一个 Connection：

- 选择尚未连接的内置 provider 后，会进入 Connection 设置页。模板 Base URL
  是默认值，连接前可以修改；需要时再填写 API key 以创建 Connection。密钥会写入
  `auth.json`。
- 选择已经 Connected 的内置 provider 会进入这个 Connection 已保存的模型列表。
  选择已有模型可以继续配置；也可以进入自定义模型卡片添加模型，或选中已有模型
  按 d/Delete 将它从这个 Connection 中移除。这个操作不会修改 provider 模板或
  内置模型目录。
- Connected provider 的 API key 和 Base URL 不能在此流程中修改。需要更换密钥或
  endpoint 时，先断开 Connection，再重新连接模板。
- 选择自定义 provider 会进入可编辑的 Connection 设置，因为它的名称、endpoint、
  协议、模型和 credential 都由用户拥有。
- Provider 选择页中，`Connected` 表示用户已有 Connection，`Template` 表示
  仍未连接的目录项。选中 Connected 项后按 `d` 或 `Delete`，确认后即可断开
  Connection。断开会移除用户 provider 覆盖和未被其他 Connection 共用的
  credential，但内置模板仍会保留。
- `Add custom provider` 会创建一个自定义 Connection。它的 provider id、
  endpoint、协议、模型和 credential 都由用户提供；以后同样通过
  `d`/`Delete` 断开，而不是删除目录文件。

因此，“provider 目录”和“provider Connection”是两个不同概念：目录可以用
git 跟踪，Connection 和 `auth.json` 则属于用户配置。

## 接入自有 API key

在 `providers.json` 中填写 credential id，在用户级 `auth.json` 中填写实际密钥：

```json
{
  "provider": {
    "my-provider": {
      "base_url": "https://api.example.com/v1",
      "credential": "my_provider_api_key",
      "models": {"my-model": {"name": "My Model"}}
    }
  },
  "model": "my-provider/my-model"
}
```

`~/.devo/auth.json`（Windows 为 `C:\Users\yourname\.devo\auth.json`）：

```json
{
  "version": 1,
  "credentials": {
    "my_provider_api_key": {
      "kind": "api_key",
      "value": "sk-your-key"
    }
  }
}
```

`devo onboard` 以及 Desktop/TUI 的 provider 流程会写入这两个文件。两处的
credential id 必须完全一致。不要把 `apiKey`、`api_key` 或密钥写进
`providers.json`；它们不是规范 provider 字段。`auth.json` 属于用户，不应提交到 git。

`auth.json` 字段如下：

| 字段 | JSON 类型 | 含义 |
| --- | --- | --- |
| `version` | 整数 | 凭据文件 schema 版本，目前为 `1`。 |
| `credentials` | 对象 | credential id 到凭据记录的映射。 |
| `credentials.<id>.kind` | 枚举 | 目前只支持 `api_key`。 |
| `credentials.<id>.value` | 字符串 | 实际 API key。 |

如果 onboarding 收到 API key 但没有明确的 credential id，会根据 provider id
生成稳定 id，例如 `deepseek_api_key`。缺少 `auth.json` 会按空凭据文件处理；引用了
不存在的 id 则会报错。

### 完整示例：自定义模型参数 + 自有 API key

下面同时配置自定义 DeepSeek 模型（Anthropic Messages）、provider 端点，以及存放在
`auth.json` 中的 API key。

`~/.devo/providers.json`（Windows 为 `C:\Users\yourname\.devo\providers.json`）：

```json
{
  "model": "deepseek/my-deepseek",
  "provider": {
    "deepseek": {
      "name": "DeepSeek Anthropic Compatible",
      "base_url": "https://api.deepseek.com/anthropic",
      "credential": "deepseek_api_key",
      "wire_api": "anthropic_messages",
      "models": {
        "my-deepseek": {
          "name": "DeepSeek V4 Flash",
          "channel": "Custom",
          "context_window": 200000,
          "effective_context_window_percent": 95,
          "max_tokens": 8192,
          "temperature": 0.2,
          "reasoning_capability": {"togglewithlevels": ["high", "max"]},
          "reasoning_implementation": "request_parameter",
          "input_modalities": ["text", "image"]
        }
      }
    }
  }
}
```

`~/.devo/auth.json`：

```json
{
  "version": 1,
  "credentials": {
    "deepseek_api_key": {
      "kind": "api_key",
      "value": "sk-deepseek-your-api-key"
    }
  }
}
```

规则：

- `provider.<id>.credential` 是引用；密钥值只能放在 `auth.json` 的同名 id 下。
- `auth.json` 不要提交到 git；git 跟踪的 `crates/core/providers.json` 不包含用户凭据。

## Provider/model JSON 配置参考

规范配置文件是 JSON。根对象包含以下字段：

### 根字段

| 字段 | JSON 类型 | 默认值 | 含义 |
| --- | --- | --- | --- |
| `model` | 字符串 | 第一个启用的模型 | 当前主模型，格式为 `provider/model`。 |
| `small_model` | 字符串 | 同 provider 的自动轻量模型，最后回退到 `model` | 标题生成等轻量后台任务使用的低成本模型；无效值也会走回退逻辑。 |
| `reasoning_effort` | 字符串 | 模型默认值 | 全局逻辑选择：`default`、`off`、`on`，或当前模型支持的 effort。旧值 `disabled`/`enabled` 读取时规范化为 `off`/`on`。 |
| `provider` | 对象 | `{}` | provider id 到 provider 记录的映射。规范键名是单数 `provider`；读取时兼容复数 `providers`。 |

provider 记录和 model 记录内部的字段都不是强制的。自定义模型甚至可以只写
map key，Devo 会补充安全的运行时默认值。provider id 和 model id 都是 map key，
不需要再重复写一份：

```json
{
  "model": "my-provider/my-model",
  "provider": {
    "my-provider": {
      "models": {
        "my-model": {}
      }
    }
  }
}
```

### Provider 字段

provider 记录位于 `provider.<provider-id>`：

`<provider-id>` map 键是 `provider/model` 中稳定的 provider 身份；`name` 只是显示名称。
重命名 provider 时不要修改这个 id。

| 字段 | JSON 类型 | 默认值 | 含义 |
| --- | --- | --- | --- |
| `name` | 字符串 | provider id | provider 的显示名称。 |
| `base_url` | 字符串 | 无 | API 端点基础 URL，应使用所选 wire API 要求的端点形式。 |
| `credential` | 字符串 | 无 | 在用户级 `auth.json` 中查找的凭据 id；密钥值不存储在 `providers.json`。 |
| `headers` | 字符串到字符串的对象 | 无 | 发送给 provider 的字面量 HTTP headers。不要在这里放 API key。 |
| `options` | JSON 对象 | 无 | provider 专用选项；内置适配器会把它们转发到请求体，模型/variant 的同名值可以覆盖它们。 |
| `request` | JSON 对象 | 无 | provider 级请求体默认值，会在 model 和 variant 之前递归合并。 |
| `wire_api` | 枚举 | `openai_chat_completions` | 该 provider 所有模型的默认请求协议。 |
| `enabled` | 布尔值 | `true` | provider 及其模型是否可被选择。 |
| `env` | 字符串数组 | `[]` | 集成可用于获取 provider 凭据的环境变量名。 |
| `web_search` | 对象 | 无 | provider 级网页搜索能力配置。 |
| `web_fetch` | 对象 | 无 | provider 级 URL 抓取能力配置。 |
| `models` | 对象 | `{}` | provider-facing model id 到模型记录的映射。 |

provider 的 `headers` 是键和值都为字符串的 JSON 对象，例如
`{ "X-Organization": "my-team" }`。`env` 只是为集成
记录可用的环境变量名；普通 provider resolver 不会自动把这些变量读取为 API key。

在 provider 记录内部，网页搜索和 URL 抓取字段可以这样写：

```json
{
  "web_search": {
    "mode": "provider"
  },
  "web_fetch": {
    "mode": "local"
  }
}
```

`web_search.mode` 可取 `disabled`、`provider`、`local`；可选的
`local_provider` 选择一个命名的本地搜索服务，`local_providers` 用来定义这些服务。
`web_fetch.mode` 可取 `disabled`、`provider`、`local`。搜索默认模式是 `provider`，
抓取默认模式是 `local`。

### Model 字段

模型位于 `provider.<provider-id>.models.<model-id>`。内部 map key 是发送给 provider
的模型 id，也是公开 `provider/model` 引用的后半部分。规范格式故意不再提供
`model_slug`、`model_name`、`model_id` 或 model `description` 字段。

| 字段 | JSON 类型 | 默认值 | 含义 |
| --- | --- | --- | --- |
| `name` | 字符串 | model id | 模型选择器中的人类可读名称。 |
| `wire_api` | 枚举 | provider 的 `wire_api` | 该模型的请求协议覆盖项。 |
| `context_window` | 整数 | 运行时默认值 | 最大上下文窗口 token 数。 |
| `effective_context_window_percent` | 数字 | 运行时默认值 | `context_window` 中视为可用的百分比（可为小数）。 |
| `max_tokens` | 整数 | 运行时默认值 | 默认响应输出上限。 |
| `temperature` | 数字 | 无 | 采样随机性。 |
| `top_p` | 数字 | 无 | nucleus sampling 概率质量。 |
| `top_k` | 数字 | 无 | 候选 token 数量上限。 |
| `reasoning_capability` | 枚举或对象 | `unsupported` | 向用户展示的 reasoning 选项。 |
| `reasoning_implementation` | 枚举或对象 | 支持 reasoning 时为 `request_parameter` | 如何将 reasoning 选择转换为请求。 |
| `default_reasoning_effort` | 枚举 | 无 | 支持多级 reasoning 时的初始 effort。 |
| `base_instructions` | 字符串 | 内置/默认指令 | 模型基础指令；显式空字符串表示禁用。 |
| `input_modalities` | 枚举数组 | `["text"]` | 输入类型，可取 `text` 和/或 `image`。 |
| `channel` | 字符串 | 无 | 模型选择器中的可选分组标签。 |
| `truncation_policy` | 对象 | `{"mode":"bytes","limit":8000}` | 过大工具结果内容的限制。 |
| `supports_image_detail_original` | 布尔值 | `false` | 是否支持原始分辨率图像细节。 |
| `enabled` | 布尔值 | `true` | 模型是否可被选择。 |
| `priority` | 整数 | `0` | 未明确指定模型时，数值越大越优先。 |

模型还支持更丰富的元数据和请求控制：

| 字段 | JSON 类型 | 含义 |
| --- | --- | --- |
| `family` | 字符串 | 用于分组和未来能力启发式判断的模型家族。 |
| `release_date` | 字符串 | provider 目录中的发布日期，通常使用 ISO-8601 文本。 |
| `status` | 字符串 | provider 报告的状态，例如 `active`、`deprecated`、`preview`。 |
| `cost` | 对象 | 开放式价格元数据；Devo 原样保存，不解释 provider 专用字段。 |
| `metadata` | 对象 | 开放式目录元数据，包括动态发现返回的原始字段。 |
| `options` | 对象 | 任意 provider/SDK 选项；内置 HTTP 适配器会把它们并入请求默认值。 |
| `request` | 对象 | 任意请求体字段；会覆盖 provider/model 中同名 option。 |
| `headers` | 字符串到字符串的对象 | model 级 HTTP headers，叠加在 provider headers 之上。API key 仍应放在 `auth.json`。 |
| `variants` | 对象 | 命名 variant map；用于 effort 编码时 key 应为逻辑选择（`off`/`on`/levels）。value 可包含 `label`、`disabled`、`request_model`、`options`、`request`、`headers`。 |
| `default_variant` | 字符串 | turn 没有显式选择、且没有 effort 匹配的 variant 时使用的静态 fallback key。 |

Variant 使用如下结构：

```json
{
  "models": {
    "reasoning-model": {
      "family": "reasoning",
      "variants": {
        "fast": {
          "label": "Fast",
          "options": {"thinking": {"budget": 1024}},
          "request": {"speed": "fast"},
          "headers": {"X-Mode": "fast"}
        }
      },
      "default_variant": "fast"
    }
  }
}
```

合并顺序为 provider `options` → provider `request` → model `options` → model
`request` → variant `options` → variant `request`。headers 也按同样的具体性
顺序叠加。`disabled: true` 会保留 variant 目录记录以保证可复现，但未来支持
variant 选择的客户端不会允许选中它。

有效上下文窗口的公式是
`context_window * effective_context_window_percent / 100`。该值是占用条与自动
压缩使用的**已应用**可用窗口（未设置百分比时默认为 95）。

请区分这两个数：

| 概念 | 在哪里改 | DeepSeek V4 Flash 例子 |
| --- | --- | --- |
| 模型硬窗口 `context_window` | 目录 / 发现（不作为单独旋钮编辑） | `1000000`（1M） |
| 可用 Context window | Desktop / TUI 模型设置 → **Context window** | 用户输入 `250000` → 存为百分比 `25` → 应用 `250000` |

Desktop / TUI 模型编辑器以绝对 token 数显示可用窗口。保存时保留硬
`context_window`，并将 `effective_context_window_percent` 设为
`clamp(user_tokens × 100 / hard, 1..=100)`（允许小数）。清空字段会去掉百分比覆盖，
恢复默认 95%。自定义模型尚无硬窗口时，输入值会把 `context_window` 设为该值，
百分比设为 100。

`config.toml` 中遗留的 `compaction_token_limit` 仍可加载以保持兼容，但对占用
与自动压缩**已忽略**。不再有单独的自动压缩阈值 UI。

### Wire API 可选值

`wire_api` 可以写在 provider 或 model 上；model 的值会覆盖 provider 的值。
只有以下三种值：

| 值 | 请求协议 | 适用情况 |
| --- | --- | --- |
| `openai_chat_completions` | OpenAI 兼容 Chat Completions | 端点接受 chat-completions 请求。 |
| `openai_responses` | OpenAI 兼容 Responses | 端点接受 Responses API 请求。 |
| `anthropic_messages` | Anthropic 兼容 Messages | 端点接受 Anthropic Messages 请求。 |

省略时使用 `openai_chat_completions`。应根据 provider 的 API 文档选择它；不能只
根据 URL 判断协议。

### 动态发现模型

Native `provider/discover` 会刷新一个已经建立的 Connection 的模型目录。它会从用户级
`auth.json` 读取 `credential` 指向的凭据，然后尝试 Connection 基础 URL 的 `/models`
以及兼容的 `/v1/models` 端点。成功返回 OpenAI 风格的 `{"data":[...]}` 或 provider
风格的 `{"models":[...]}` 后，结果会被规范化为模型 map，并写入用户级
`providers.json` 覆盖层。传入 `{"forceRefresh":true}` 可以绕过进程内的短期缓存。

动态发现是增量操作：它会更新返回中的模型记录，同时把 provider 原始条目保存在
`metadata` 中；git 跟踪的内置目录不会被修改。若返回中存在，`id`、`name`、`family`、
`status`、发布日期、上下文上限、输出上限、价格、reasoning 和输入模态等常见字段会被
规范化。没有模型目录端点的 provider 可以直接在 `models` map 中填写自定义模型。

### Reasoning 字段

`reasoning_capability` 决定 UI 展示哪些选择，精确支持以下三种 JSON 形式：

| JSON 值 | 含义 |
| --- | --- |
| `"unsupported"` | 不展示 reasoning 控件。 |
| `"toggle"` | 展示 `off` 和 `on`。 |
| `{ "levels": ["off", "low", "high"] }` | 精确展示数组中的芯片。数组含 `off` 时允许关闭；不含 `off` 表示无法关闭 reasoning。 |

旧写法 `{"toggle_with_levels":[...]}` 仍可读取，并迁移为带前导 `off` 的 `levels`。

effort 字符串完整取值为 `none`、`minimal`、`low`、`medium`、`high`、`xhigh`、
`max`。数组只应填写该 provider 模型实际支持的值。`default_reasoning_effort` 使用
同一组 effort 值；`unsupported` 模型不应设置它。`default_reasoning_selection`
保存确切的逻辑选择（`off`、`on` 或某个 level）。读取时仍接受旧字面量
`disabled`/`enabled`，并规范化为 `off`/`on`。

会话与 composer UI 始终只选择来自 `reasoning_capability` 的**逻辑**档位。该档位
如何编码到请求上，是 **Connection 上该模型** 的配置，因此同一上游模型在不同部署
下可以不同：

| 模式 | 何时 | 行为 |
| --- | --- | --- |
| Adapter | catalog `variants` 中没有与选择匹配的 key | 内置 adapter 填写一等字段 `thinking` / `reasoning_effort`。 |
| CatalogVariant | `variants` 存在与选择相同的 key（`off`/`on`/levels；旧 key `disabled`/`enabled` 也可匹配） | 清空一等 thinking/effort 字段；合并该 variant 的 `request` / `options` / `headers` / 可选 `request_model`。 |

请把 variant key 命名为逻辑选择值。仅用 JSON 编码 effort 的自定义网关示例：

```json
{
  "reasoning_capability": {"levels": ["low", "medium", "high"]},
  "default_reasoning_selection": "medium",
  "variants": {
    "low": {"request": {"ext": {"effort": "L"}}},
    "medium": {"request": {"ext": {"effort": "M"}}},
    "high": {"request": {"ext": {"effort": "H"}}}
  }
}
```

通过 `request_model` 切换 wire 模型 id 的示例（替代旧的
`reasoning_implementation: model_variant`）：

```json
{
  "reasoning_capability": "toggle",
  "variants": {
    "off": {"request_model": "deepseek-chat"},
    "on": {"request_model": "deepseek-reasoner"}
  }
}
```

`reasoning_implementation` 仅为旧 TOML 迁移保留；当 `variants` 为空时会投影进
`variants`。新的 JSON 配置应使用 `reasoning_capability` 配置选择器，并用命名
`variants` map 配置编码。Desktop 与 TUI 都可编辑这些模型字段；日常选择器只展示
capability 推导出的档位。

Desktop SDK 注意：chat composer 里的合成 `variants` 列表是逻辑 effort 选项值
（来自 `availableEfforts`），不是 catalog 的 `variants` map。catalog 编码仍挂在
模型记录上。

`truncation_policy` 使用以下结构：

```json
{
  "truncation_policy": {
    "mode": "tokens",
    "limit": 12000
  }
}
```

`mode` 可取 `bytes` 或 `tokens`，`limit` 是整数。

### 覆盖和自定义模型规则

用户级和工作区级文件按配置顺序覆盖 git 跟踪的内置目录。重复相同的 provider/model
键时，只有高优先级文件中出现的字段会覆盖旧值。新增 provider key 会创建自定义
provider；新增嵌套 model key 会创建带安全默认值的自定义模型。通过
`provider/model` 引用选择它：

```json
{
  "model": "example/custom",
  "provider": {
    "example": {
      "name": "Example Gateway",
      "base_url": "https://api.example.com/v1",
      "credential": "example_api_key",
      "wire_api": "openai_chat_completions",
      "models": {
        "custom": {
          "name": "Example Custom Model",
          "context_window": 128000,
          "input_modalities": ["text"]
        }
      }
    }
  }
}
```

省略模型元数据是合法的。覆盖内置模型时，省略的字段保持不变；自定义模型省略的
字段使用 Devo 默认值。旧 TOML 的标量、provider、binding 和 model override 字段
仅为迁移保留。

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
- 若仍存在遗留的 `compaction_token_limit`，会被忽略；请在 Settings › Models 中
  为各模型设置可用 Context window。

### 从旧配置迁移

git 跟踪的 `crates/core/providers.json` 现在是 Devo 的内置 provider/model 目录。
启动时，Devo 读取用户级或工作区 `config.toml` 后，会在解析当前模型之前，
自动把旧的 provider、model、binding 和模型选择配置迁移到对应的 `providers.json`：

- 用户级配置从 `DEVO_HOME/config.toml` 迁移到 `DEVO_HOME/providers.json`。
- 工作区配置从 `<workspace>/.devo/config.toml` 迁移到
  `<workspace>/.devo/providers.json`。
- 已存在的 JSON 字段优先，因此较新的 JSON 配置不会被旧 TOML 覆盖。
- 旧 API key 会复制到用户级 `auth.json`，`providers.json` 只保存 credential id
  引用，绝不会写入 API key 值。
- 只删除 provider 所有的旧 TOML 字段；其他应用配置会保留。如果旧的
  `[model.<name>]` 无法安全关联到某个 provider 模型，则会继续保留，避免丢失配置。

迁移是幂等的：首次成功启动后，后续启动直接使用 JSON 目录。新的 onboarding
以及 provider/model 写入也都会使用 `providers.json`。

## MCP 服务器

Devo 通过用户或工作区 `config.toml` 中的 `[mcp]` 配置
[Model Context Protocol](https://modelcontextprotocol.io/) 服务器。每个服务器是
`servers` 数组中的一项，其 `transport` 表决定 Devo 的连接方式。支持的传输方式有
`stdio`、`streamable_http` 和已弃用的 `sse`。

可以用编辑 `config.toml` 或 CLI（`devo mcp …`）两种方式配置 MCP。日常的添加 /
启用 / 禁用 / 删除优先用 CLI；需要细调传输参数、环境变量或 header 时再改 TOML。

### 捆绑的 `code_search`（默认关闭）

Devo 会在 `devo` 旁边安装可选的语义搜索 MCP 二进制。配置项在缺失时会自动注入，
且保持 **disabled**，直到你显式启用：

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
# 或在交互会话中：/mcps → Code Search → Enable
```

启用后，模型侧工具名为 `mcp__code_search__code_search`。

### CLI 管理

用 `devo mcp` 管理用户级 MCP 服务器（`~/.devo/config.toml`）：

```bash
# 列出已配置服务器
devo mcp list

# 添加 stdio 服务器（`--` 后为 command + args）
devo mcp add time -- docker run -i --rm mcp/time
devo mcp add filesystem --env HOME=/tmp -- npx -y @modelcontextprotocol/server-filesystem .

# 添加 Streamable HTTP（`--transport http` 写入 kind = "streamable_http"）
devo mcp add --transport http hello-mcp http://localhost:8080/mcp
devo mcp add --transport http github --bearer-token "$TOKEN" https://api.githubcopilot.com/mcp/

# 添加旧版 SSE
devo mcp add --transport sse legacy-mcp https://example.com/mcp/sse

# 按 id 启用 / 禁用 / 删除
devo mcp enable time
devo mcp disable time
devo mcp remove time
```

CLI `devo mcp enable|disable` 会写入用户 `config.toml`（离线配置）。已在运行的
交互会话通过 TUI `/mcps`（`mcp/set_enabled` RPC）即时启用/禁用。

可在 TUI 中用 `/mcps`（交互式列表 → 详情 → 工具）验证配置。客户端也可调用
`mcp/list`、`mcp/tools`、`mcp/set_enabled`。

### TOML 示例

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
