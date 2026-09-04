# 設定

[English](./configuration.md) | [简体中文](./configuration.zh-Hans.md) | [繁體中文](./configuration.zh-Hant.md) | [日本語](./configuration.ja.md) | [Русский](./configuration.ru.md)

`devo onboard` が推奨されるセットアップ方法です。provider と model は独立した
`providers.json` に保存し、API key はユーザースコープの `auth.json` にだけ保存します。
旧版 `config.toml` の provider/model 設定は起動時に自動移行され、成功後に旧フィールドが削除されます。

## ファイルと優先順位

設定は組み込みデフォルト、`DEVO_HOME/config.toml`、`DEVO_HOME/providers.json`、
ワークスペースの `.devo/config.toml`、`.devo/providers.json`、CLI flags の順で
マージされます。git で管理される組み込みディレクトリは
`crates/core/providers.json`、ユーザーと workspace のファイルは overlay です。

基本形は次のとおりです。

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

provider id と model id はそれぞれ map key です。安定したモデル識別子は
`provider/model` だけで、binding id、local slug、request model、model description
を別に持つ必要はありません。

`model` は通常の turn 用の主モデルです。`small_model` はセッションタイトルなど
軽量なバックグラウンド処理用の低コストモデルです。省略時は同じ provider の
flash、mini、small、nano などを探し、最後に主モデルへ戻ります。無効な値も同じように
フォールバックします。

## auth.json

`providers.json` には credential id だけを書き、実際の秘密値はユーザーの
`DEVO_HOME/auth.json` に置きます。git にコミットしないでください。

```json
{
  "version": 1,
  "credentials": {
    "my_provider_api_key": { "kind": "api_key", "value": "sk-your-key" }
  }
}
```

現在の credential kind は `api_key` のみです。`credential` は
`credentials.<id>` と完全一致させます。`providers.json` に `apiKey`、`api_key`、
または秘密値を書かないでください。

## Provider template と Connection

組み込み provider は git 管理される読み取り専用テンプレートで、ログイン済みとは
限りません。テンプレートを確認すると、ユーザーの `providers.json` に Connection
が作成されます。組み込み template の名前、Base URL、wire API はこの画面から変更
できません。key や endpoint を変えるときは Connection を切断して再接続します。
カスタム provider の名前、endpoint、protocol、model はユーザー所有なので編集できます。
接続済み provider では保存済み model を管理でき、`d` または Delete で Connection を
切断できます。これは組み込みテンプレートを削除しません。

provider map に新しい key を追加すればカスタム provider、`models` map に新しい key を
追加すればカスタム model になります。

## フィールドリファレンス

Provider の主なフィールドは `name`、`base_url`、`credential`、`wire_api`、`enabled`、
`headers`、`options`、`request`、`models` です。`headers` は文字列から文字列への
JSON object で、API key はここに置きません。

Model の map key が provider に送る model id です。`name`、`wire_api`、
`context_window`、`effective_context_window_percent`、`max_tokens`、`temperature`、
`top_p`、`top_k`、`input_modalities`（`text`/`image`）、`base_instructions`、
`enabled`、`priority`、`family`、`release_date`、`status`、`cost`、`metadata`、
`headers`、`options`、`request` を指定できます。

`reasoning_capability` は `unsupported`、`toggle`、
`{"levels":["off","low","high"]}` のいずれかです（`off` を含めると無効化可能、
含めないと常時オン）。旧形式 `{"toggle_with_levels":[...]}` は読み取り時に
先頭 `off` 付きの `levels` へ移行します。effort は `none`、
`minimal`、`low`、`medium`、`high`、`xhigh`、`max`。`reasoning_implementation` は
旧 TOML の移行用で、新しい JSON では named variants を推奨します。

Variant は model 内の named mode です。

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

`wire_api` は `openai_chat_completions`、`openai_responses`、`anthropic_messages` の
3 種類です。model の値は provider の値を上書きし、省略時は
`openai_chat_completions` になります。DeepSeek の組み込み template は公式の
Anthropic 互換 endpoint を使用します。

## 動的モデル発見

接続済み Connection に Native `provider/discover` を実行すると、`auth.json` の key で
`/models` または `/v1/models` を試し、検出結果を Connection の model map に保存します。
`{"forceRefresh":true}` で短期キャッシュを無視できます。

全フィールドの表、web search/fetch、移行の詳細は[英語の設定リファレンス](./configuration.md)を参照してください。
