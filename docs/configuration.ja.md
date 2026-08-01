# 設定

[English](./configuration.md) | [简体中文](./configuration.zh-Hans.md) | [繁體中文](./configuration.zh-Hant.md) | [日本語](./configuration.ja.md) | [Русский](./configuration.ru.md)

`devo onboard` が推奨されるセットアップ方法です。手動で設定する場合、Devo は次の順序で設定をマージします:

1. 組み込みデフォルト
2. `DEVO_HOME/config.toml` - ユーザーレベル設定。デフォルトでは macOS/Linux で
   `~/.devo/config.toml`、Windows で `C:\Users\yourname\.devo\config.toml`
3. `<workspace>/.devo/config.toml` - プロジェクトレベル設定
4. CLI flags

認証情報は `DEVO_HOME/auth.json` に分離して保存されます。
`config.toml` には API key を直接保存せず、credential id を参照させてください。

最小構成（組み込みモデル + provider バインディング）:

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

重要な分離は次のとおりです:

- `model_slug` は slug で Devo のローカルモデルメタデータを選択します。
- binding の `provider` は `[providers.<id>]` 接続レコードを選択します。
- `request_model` はプロバイダーへ送信されるモデル id です。
- `invocation_method` は実際に使うプロバイダープロトコルを選択します。詳細は
  [呼び出し方式（Invocation methods）](#呼び出し方式invocation-methods) を参照してください。

モデルメタデータにも `provider` フィールドがあり、モデルが期待する wire API を
表します。binding の `invocation_method` は実行時の接続方法を選ぶため、両者を一致
させてください。API key は引き続き `auth.json` に保存し、provider の `credential`
参照で接続します。

既存の `model_name` 設定は引き続き読み取れます。次回その binding を保存すると、
Devo は `request_model` として書き出します。

## 自分の API key を使う

Devo は API key を `config.toml` に保存しません。自分の key を使う場合:

1. シークレットをユーザースコープの `DEVO_HOME/auth.json` に保存します。
2. `config.toml` の `[providers.<id>].credential` からその credential id を参照します。

`devo onboard` と Desktop/TUI の provider フローが両ファイルを書き込みます。

### エンドツーエンド例: カスタムモデルパラメータ + 自分の API key

次の例は、カスタム DeepSeek モデル（Anthropic Messages）、provider
エンドポイント、および `auth.json` のみに置く credential を組み合わせます。

`~/.devo/config.toml`（Windows では `C:\Users\yourname\.devo\config.toml`）:

```toml
[defaults]
model_binding = "deepseek-example"

[model.my-deepseek]
display_name = "DeepSeek V4 Flash"
description = "Custom Anthropic Messages coding model for DeepSeek."
channel = "Custom"
# このモデルが期待する wire API。binding の invocation_method と一致させる。
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
# credential id のみ — シークレットは auth.json に置く。
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

対応する `~/.devo/auth.json`（Windows では `C:\Users\yourname\.devo\auth.json`）:

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

ルール:

- 現時点でサポートされるのは `api_key` 認証情報のみです。
- credential id は `[providers.<id>].credential` と完全に一致する必要があります。
- `auth.json` は `DEVO_HOME` 配下に置き、プロジェクトリポジトリへコミットしないでください。
- ワークスペース `<workspace>/.devo/config.toml` は credential id を参照できますが、
  シークレット値はユーザースコープの `auth.json` にのみ置きます。
- key だけ更新する場合は `auth.json` を編集します。credential id が同じなら
  `config.toml` は変更不要です。

## 呼び出し方式（Invocation methods）

binding の `invocation_method` と provider の `wire_apis` が、Devo が使う HTTP API
を決めます。モデルメタデータの `provider` も同じ値にして、カタログ能力と実行時接続を
揃えてください。

| 値 | プロトコル | 典型的なエンドポイント |
| --- | --- | --- |
| `openai_chat_completions` | [OpenAI Chat Completions](https://developers.openai.com/api/reference/chat-completions/overview) | 多くの OpenAI 互換ゲートウェイ（DeepSeek、Qwen、Kimi、OpenRouter、ローカルプロキシなど） |
| `openai_responses` | [OpenAI Responses](https://developers.openai.com/api/reference/responses/overview) | Responses API を提供するサービス |
| `anthropic_messages` | [Anthropic Messages](https://platform.claude.com/docs/en/api/messages) | Anthropic 互換 Messages エンドポイント |

## モデルメタデータとカスタムモデル

ユーザーまたは workspace の `config.toml` の `[model.<slug>]` で設定します。
組み込み slug は部分上書きで、省略したフィールドは組み込み値を保持します。新しい
slug は安全なデフォルトを持つカスタムモデルを作成し、
[エンドツーエンド例](#エンドツーエンド例-カスタムモデルパラメータ--自分の-api-key)
のように `[providers.<id>]` と `[model_bindings.<id>]` で接続します。

組み込みモデルの部分上書き例:

```toml
[model.qwen3-coder-next]
context_window = 262144
effective_context_window_percent = 90
```

有効なコンテキストウィンドウの正確な式は
`context_window * effective_context_window_percent / 100` です。その結果がモデルで
利用可能なコンテキストであり、自動 compaction の境界でもあります。

設定可能なメタデータには、ピッカー向けの `display_name`、説明文の `description`、
グループ用の `channel` があります。`context_window` と
`effective_context_window_percent` が有効コンテキストを決め、`max_tokens` は既定の
出力上限です。サンプリング既定値は `temperature`（乱数性）、`top_p`（核サンプリング）、
`top_k`（候補トークン上限）です。`provider` wire API は
`openai_chat_completions`、`openai_responses`、`anthropic_messages` のいずれかです。
推論メタデータは型付きで、`reasoning_capability` は `unsupported`、`toggle`、
`{ levels = [...] }`、`{ togglewithlevels = [...] }`、
`reasoning_implementation` は `disabled`、`request_parameter`、または型付き
`model_variant` テーブルです。`model_variant` は論理的な推論選択を別の
provider 向けモデル id、任意の有効 effort、任意の追加リクエスト本文へ写像します。
`default_reasoning_effort` は既定の effort を選びます。`input_modalities` は
`text` と `image` を受け付け、`truncation_policy` は大きすぎるツール結果の
バイト/トークン上限を選び、`supports_image_detail_original` は元解像度の画像詳細を
有効にします。

`base_instructions` を省略すると、組み込みモデルは組み込み指示を、カスタムモデルは
Devo の既定指示を使います。明示的な空文字（`base_instructions = ""`）は指示なしを
意味します。

レガシーのスカラー `model = "slug"` は引き続き読み取れます。`[model.<slug>]` が
トップレベル `model` テーブル名前空間を占有するため、新しい設定では
`[defaults].model_binding` でアクティブ接続を選んでください。

### TUI 設定

`DEVO_HOME/config.toml` のトップレベルには UI 設定も保存されます:

```toml
theme = "aurora"
collapse_reasoning = true
```

- `theme` は TUI の配色テーマを選びます（`/theme` でも設定可）。
- `collapse_reasoning` は推論表示を制御します（`/show-reasoning` でも設定可）:
  - `true`（既定）: ストリーミング中は最新 3 行のみ。完了後は短い推論を全文表示し、
    長い推論は 1 行の `Thought · …` 要約に折りたたみます（全文は Ctrl+T で確認可）。
  - `false`: ストリーミング中も完了後も全文を表示します。

### `models.json` からの移行

古い `~/.devo/models.json` と `<workspace>/.devo/models.json` は無視されます。
必要なフィールドをユーザーまたは workspace の `config.toml` の
`[model.<slug>]` へ手動でコピーし、対応する provider と model binding を追加または
保持してください。API key は `auth.json` に置き、`[providers.<id>].credential`
から参照します。

## MCP サーバー

Devo は、ユーザーまたは workspace の `config.toml` の `[mcp]` で設定した
[Model Context Protocol](https://modelcontextprotocol.io/) サーバーに接続します。
各サーバーは `servers` 配列の 1 エントリで、`transport` テーブルが接続方式を
決めます。対応トランスポートは `stdio`、`streamable_http`、非推奨の `sse` です。

stdio の例:

```toml
[mcp]
auto_start = true
refresh_on_config_reload = true

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

bearer token を使う Streamable HTTP:

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

レガシー SSE トランスポート:

```toml
[mcp.servers.transport]
kind = "sse"
url = "https://example.com/mcp/sse"
```

フィールドの説明:

- `auto_start` と `refresh_on_config_reload` は既定で `true` です。
- `startup_policy` は有効なサーバーの起動タイミングを制御します: `eager` は
  ブートストラップ時、`lazy` は初回利用時、`manual` は明示的な要求のみです。
- stdio では `env` がリテラル値を渡し、`env_vars` がローカル環境から継承する
  変数名のリストです。stdio では `{ name = "X", source = "remote" }` は
  サポートされません。
- HTTP トランスポートでは、`http_headers` がリテラルヘッダーを渡し、
  `env_http_headers` はヘッダー名を、値を供給する環境変数名に対応付けます。
- `allowed_capabilities` が空の場合は制限なしです。現時点のランタイムは主に
  `tools` を扱い、リソース読み取りはまだ接続されていません。
- `output_limits` は `max_tool_output_bytes`（既定 1 MiB）と
  `max_resource_bytes`（既定 10 MiB）を設定します。
- トップレベルの `mcp_oauth_credentials_store` は `auto`（既定）、`file`、
  `keyring` のいずれかで、OAuth 認証情報の保存先を選びます。
- token を `config.toml` に直接書くより、環境変数からヘッダーや値を注入する
  ことを推奨します。`auth_ref` は各サーバーレコードに存在しますが、ランタイム
  にはまだ接続されていません。

マージ動作: `[mcp]` は他のテーブルと同じくフィールド単位でマージされますが、
`servers` は配列です。したがって、workspace の `[[mcp.servers]]` リストは
ユーザーレベルのリストを `id` 単位でマージせず置き換えます。

TUI の `/mcps` で対話的に確認できます（一覧 → 詳細 → ツール。Enable/Disable は設定のみ更新し、実行時反映にはセッション再起動が必要な場合があります）。クライアントは `mcp/list` / `mcp/tools` RPC も利用できます。ユーザー設定 (`~/.devo/config.toml`) は
`devo mcp add|list|remove|enable|disable` でも管理できます（`--transport stdio|http|sse`）。
クライアントは `mcp/list` RPC でランタイム状態を取得できます。
