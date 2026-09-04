# 配置

[English](./configuration.md) | [简体中文](./configuration.zh-Hans.md) | [繁體中文](./configuration.zh-Hant.md) | [日本語](./configuration.ja.md) | [Русский](./configuration.ru.md)

`devo onboard` 是推薦的設定路徑。Provider 與 model 現在使用獨立的
`providers.json`；API key 只保存在使用者級 `auth.json`。舊版
`config.toml` 中的 provider/model 設定在啟動時會自動遷移，成功後會移除舊欄位。

## 檔案與優先順序

設定會依序合併：內建預設、`DEVO_HOME/config.toml`、
`DEVO_HOME/providers.json`、工作區 `.devo/config.toml`、工作區
`.devo/providers.json`，最後是 CLI flags。git 跟蹤的內建 provider 目錄是
`crates/core/providers.json`；使用者與工作區檔案是覆蓋層。

最小的 provider/model 結構如下：

```json
{
  "model": "my-provider/my-model",
  "provider": {
    "my-provider": {
      "name": "My Provider",
      "base_url": "https://api.example.com/v1",
      "credential": "my_provider_api_key",
      "wire_api": "openai_chat_completions",
      "models": {
        "my-model": { "name": "My Model", "context_window": 131072 }
      }
    }
  }
}
```

Provider id 與 model id 都是 map key，公開的穩定模型身份只有
`provider/model`。不再需要 binding id、local slug、request model 或
model description。

`model` 是一般 turn 使用的主模型。可選的 `small_model` 是標題生成等輕量
背景工作的低成本模型；未設定時先在同一 provider 自動尋找 flash、mini、
small、nano 等模型，最後回退到主模型。無效的 `small_model` 也會回退。

## auth.json

`providers.json` 只保存 credential id，實際密鑰放在使用者級
`DEVO_HOME/auth.json`，不可提交到 git：

```json
{
  "version": 1,
  "credentials": {
    "my_provider_api_key": { "kind": "api_key", "value": "sk-your-key" }
  }
}
```

目前 `kind` 只有 `api_key`。`credential` 必須精確對應
`credentials.<id>`；不要在 `providers.json` 中放 `apiKey`、`api_key` 或密鑰。

## Provider 模板與 Connection

內建 provider 是 git 跟蹤的唯讀模板，不代表已登入。選取模板並確認後，Devo
會在使用者 `providers.json` 建立 Connection。內建模板的名稱、Base URL 和
wire API 不能直接修改；要更換 API key 或 endpoint，先斷開 Connection，再
重新連接。自訂 provider 的名稱、endpoint、協議和模型則由使用者擁有，可以
編輯。選取已連接的 provider 可管理該 Connection 的模型，按 `d` 或 Delete
可斷開它；這不會刪除內建模板。

任意自訂 provider/model 都可以直接加入覆蓋層：provider map 的新 key 會建立
自訂 provider，`models` map 的新 key 會建立自訂 model。

## 欄位參考

Provider 常用欄位：`name`（顯示名稱）、`base_url`（API endpoint）、
`credential`（auth.json id）、`wire_api`（預設協議）、`enabled`、`headers`、
`options`、`request`、`models`。`headers` 是字串到字串的 JSON object，
API key 不應放在其中。

Model 的 map key 是送給 provider 的 model id。可配置：`name`、`wire_api`、
`context_window`、`effective_context_window_percent`、`max_tokens`、
`temperature`、`top_p`、`top_k`、`input_modalities`（`text`、`image`）、
`base_instructions`、`enabled`、`priority`、`family`、`release_date`、
`status`、`cost`、`metadata`、`headers`、`options`、`request`。

Reasoning 可用 `reasoning_capability`：`unsupported`、`toggle`、
`{"levels":["off","low","high"]}`（陣列含 `off` 才可關閉；省略 `off` 表示無法關閉）。
舊寫法 `{"toggle_with_levels":[...]}` 讀取時會遷移為帶前導 `off` 的 `levels`。
`default_reasoning_effort` 可取 `none`、`minimal`、`low`、`medium`、`high`、
`xhigh`、`max`。`reasoning_implementation` 是舊 TOML 遷移欄位，新 JSON
應優先使用 variants。

Variant 是 model 下的命名模式，支援 `label`、`disabled`、任意 JSON 的
`options`/`request`，以及字串 map `headers`：

```json
{
  "models": {
    "reasoning-model": {
      "variants": {
        "fast": {
          "label": "Fast",
          "options": { "thinking": { "budget": 1024 } },
          "request": { "speed": "fast" },
          "headers": { "X-Mode": "fast" }
        }
      },
      "default_variant": "fast"
    }
  }
}
```

`wire_api` 只有三種：`openai_chat_completions`、`openai_responses`、
`anthropic_messages`。model 的值會覆蓋 provider 的值；省略時使用
`openai_chat_completions`。DeepSeek 內建模板使用官方 Anthropic 相容 endpoint。

## 動態發現

對已建立的 Connection 呼叫 Native `provider/discover`，Devo 會使用
`auth.json` 中的 credential 嘗試 `/models` 或 `/v1/models`，把結果合併後寫回
該 Connection 的 `providers.json`。`{"forceRefresh":true}` 可略過短期快取。

完整欄位、web search/fetch 與遷移細節請參閱[簡體中文配置文件](./configuration.zh-Hans.md)。
