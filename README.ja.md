<div align="center">

<img src="./.github/assets/devo-readme-brand.svg" alt="Devo desktop coding agent app icon and wordmark" width="360" />

</div>

<div align="center">

**Devo は、Desktop app、terminal TUI/CLI、model-neutral な Rust runtime を備えた open-source coding agent です。プライベート、エンタープライズ、OpenAI 互換モデル環境で、DeepSeek、Qwen、Kimi、Anthropic 互換 API、ローカルゲートウェイ、独自モデル endpoint に接続できます。**

[![Stars](https://img.shields.io/github/stars/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/stargazers)
[![Language](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/7df-lab/devo/pulls)
[![CI](https://img.shields.io/github/actions/workflow/status/7df-lab/devo/ci.yml?branch=main&style=flat-square)](https://github.com/7df-lab/devo/actions)
[![Release](https://img.shields.io/github/v/release/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/releases)

[English](./README.md) | [简体中文](./README.zh-Hans.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | [Русский](./README.ru.md)

[Devo を選ぶ理由](#devo-を選ぶ理由) · [スクリーンショット](#スクリーンショット) · [機能](#機能) · [検証済みモデル](#検証済みモデル) · [検証済みプラットフォーム](#検証済みプラットフォーム) · [インストール](#インストール) · [クイックスタート](#クイックスタート) · [ドキュメント](#docs)

</div>

---

## スクリーンショット

<p align="center">
  <img width="100%" alt="Devo desktop coding agent app がリポジトリの会話、プロジェクトサイドバー、モデル制御を表示している画面" src="./.github/assets/devo-desktop-coding-agent-screenshot.png" />
</p>

<p align="center">
  <img width="100%" alt="Devo terminal TUI coding agent がローカルリポジトリでモデル、コンテキスト、token 状態を表示している画面" src="./.github/assets/devo-terminal-tui-coding-agent-screenshot.png" />
</p>

## Devo を選ぶ理由

Devo は、単一のホスト型モデルエコシステムに縛られず、モデル選択、
Desktop 体験、terminal workflow、ランタイムの動作、ワークスペースでの
実行境界を自分たちで制御したいチームのための coding agent です。

- **任意のモデルを接続** - provider/model binding により、OpenAI 互換 Chat
  Completions、OpenAI 互換 Responses、Anthropic Messages、DeepSeek、
  Qwen、Kimi、またはプライベートモデルゲートウェイを接続できます。
- **プライベート環境やイントラネット環境に対応** - 単一のローカル Rust
  バイナリとして動作し、オフラインインストール経路をサポートし、
  ホスト型 agent service に依存せず内部 endpoint を指定できます。
- **Desktop と terminal の両方に対応** - Desktop app で視覚的なオンボーディングと
  日常の coding を行い、端末ネイティブな自動化、remote shell、scriptable workflow が
  必要なときは CLI/TUI を使えます。
- **Agent Runtime として拡張可能** - MCP server、再利用可能な skills、
  ローカルのセマンティックコード検索、監査可能なセッション、権限管理、
  マルチエージェント flow は、一回限りの prompt ではなくランタイム機能です。

## 機能

- **組み込みのセマンティックコード検索** - ローカル CPU のコード埋め込みモデルを実行し、
  dense retrieval と BM25 キーワードマッチングを組み合わせることで、grep/find のみに頼るエージェントより
  コード検索のコンテキストを削減します。
- **Model-neutral provider runtime** - provider/model binding により、OpenAI 互換、
  Anthropic 互換、DeepSeek、Qwen、Kimi、GLM、MiniMax、Xiaomi MiMo、
  OpenRouter、またはローカルエンドポイントを利用できます。
- **MCP サポート** - [Model Context Protocol](https://modelcontextprotocol.io/)
  サーバーを通じて外部ツールとコンテキストを接続できます。設定方法は
  [設定](./docs/configuration.ja.md#mcp-サーバー) を参照してください。
- **Skill サポート** - 再利用可能なワークフロー、手順、スクリプト、参照資料を
  [Agent Skills](https://agentskills.io/) としてパッケージ化できます。
- **長時間タスクのサポート** - 複数ターンにまたがる作業でも Devo が自動的にコンテキストを管理し、
  タスクが大きくなっても流れを失いにくくします。
- **マルチエージェントサポート** - 専門エージェントに作業を分割しつつ、セッション内で調整状況を可視化します。
- **Plan Mode** - 実装を始める前に、大きなタスクを明確な複数ステップの計画へ分解します。
- **並列ツール呼び出し** - 独立した複数のツールを並列に実行し、モデルの待ち時間を減らして作業を進めます。
- **権限付きツール実行** - センシティブなツール呼び出しがワークスペースに触れる前にレビューできます。
- **監査可能なセッション** - モデル出力、ツール呼び出し、承認、token 使用量、セッション履歴を
  確認および再開できる形で保持します。
- **コストとコンテキストの可視化** - プロバイダーが提供する場合、入力/出力 token、cached token、
  コンテキストウィンドウ使用量を表示します。
- **軽量な Rust ランタイム** - Rust で構築され、メモリ使用量が小さく、コンパクトなローカルランタイムを備えます。

## 検証済みモデル

<p>
  <img alt="DeepSeek v4 Flash / Pro" src="https://img.shields.io/badge/DeepSeek-v4%20Flash%20%2F%20Pro-4D6BFE?style=flat-square&logo=deepseek&logoColor=white" />
  <img alt="GLM 5.2" src="https://img.shields.io/badge/GLM-5.2-7856FF?style=flat-square&logo=huggingfacei&logoColor=white" />
  <img alt="MiniMax M3" src="https://img.shields.io/badge/MiniMax-M3-0B5FFF?style=flat-square&logo=minimax&logoColor=white" />
  <img alt="Qwen3 Coder Next" src="https://img.shields.io/badge/Qwen3-Coder%20Next-615CED?style=flat-square&logo=qwen&logoColor=white" />
  <img alt="Kimi K3" src="https://img.shields.io/badge/Kimi-K3-111111?style=flat-square&logo=moonshotai&logoColor=white" />
</p>

Devo の組み込みモデルカタログには、Qwen、Kimi、MiniMax、GLM、DeepSeek の検証済みモデル定義が含まれています。
プロバイダーのエンドポイントは provider/model binding で引き続き設定できます。

## 検証済みプラットフォーム

<p>
  <img alt="macOS 検証済み" src="https://img.shields.io/badge/macOS-tested-000000?style=flat-square&logo=apple&logoColor=white" />
  <img alt="Linux 検証済み" src="https://img.shields.io/badge/Linux-tested-FCC624?style=flat-square&logo=linux&logoColor=000000" />
  <img alt="Windows 検証済み" src="https://img.shields.io/badge/Windows-tested-0078D4?style=flat-square&logo=windows&logoColor=white" />
</p>

Devo は macOS、Linux、Windows、Kylin OS で検証済みです。

### 中国のエンタープライズユーザー向け

<p>
  <img alt="Kylin OS 検証済み" src="https://img.shields.io/badge/Kylin%20OS-tested-1E88E5?style=flat-square" />
  <img alt="HarmonyOS サポートはロードマップ上" src="https://img.shields.io/badge/HarmonyOS-on%20the%20road-111111?style=flat-square&logo=harmonyos&logoColor=white" />
</p>

中国のエンタープライズ環境では国産 OS が実際のデプロイ要件になることが多いため、Kylin OS の対応状況を明記しています。
HarmonyOS サポートはロードマップ上にあります。HarmonyOS デバイスを持つコントリビューターによる、そのプラットフォーム向けのビルド、テスト、リリースを歓迎します。

## インストール

Devo には 2 つのインストール形態があります。グラフィカルな coding agent
workspace が必要なら Desktop app、shell-first な開発には端末ネイティブな
TUI/CLI を選んでください。両方を同じマシンにインストールすることもできます。

### オプション 1: Desktop App

グラフィカルな Devo 体験を使いたい場合は、まず
[GitHub Releases](https://github.com/7df-lab/devo/releases/latest)
から最新の Devo Desktop package をダウンロードし、OS とアーキテクチャに合う
asset を選んでください:

- **macOS** - `devo-desktop-...-mac-...` の `.dmg` または `.zip` asset をダウンロードします。
- **Windows** - `devo-desktop-...-windows-...` の `.exe` asset をダウンロードします。
- **Linux** - `devo-desktop-...-linux-...` の `.AppImage`、`.deb`、または
  `.rpm` asset をダウンロードします。

**macOS に「Devo」は壊れているため開けません、と表示される場合がありますが、これは想定された動作です。**
現在の macOS Desktop builds は署名されていないため、インストール後に次のコマンドを実行すると
macOS でアプリを起動できます:

```bash
sudo xattr -dr com.apple.quarantine /Applications/Devo.app
```

### オプション 2: TUI / CLI

端末 TUI を使いたい場合、shell automation が必要な場合、または Desktop app と
併用したい場合は、端末ネイティブな `devo` command をインストールしてください。

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
```

Windows:

```powershell
irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

オンラインインストーラーは `devo` を Devo home ディレクトリに配置し、高速なリポジトリ検索に使う
`rg` sidecar をインストールします。また、`code_search` が使うローカルモデルの任意設定にも対応しています。

<details>
<summary>任意: ローカルの <code>code_search</code> モデルを事前インストール</summary>

インストール時に Hugging Face モデルをダウンロードしたい場合だけ使用してください。

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh -s -- --install-code-search-model
```

Windows:

```powershell
$env:DEVO_INSTALL_CODE_SEARCH_MODEL = "1"; irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

</details>

既存のインストールを最新 release にアップグレードするには:

```bash
devo upgrade
```

アップグレードコマンドは同じプラットフォーム用インストーラーを実行し、
インストーラーは `Version: v0.1.12 -> v0.1.15` のようにバージョン遷移を表示します。

イントラネット環境やオフライン環境でインストールする場合は、
[オフラインインストール](./docs/offline-installation.ja.md) を参照してください。

## クイックスタート

プロバイダーを設定し、リポジトリを開いて TUI を起動します:

```bash
cd /path/to/your/repo
devo onboard
```

便利なコマンド:

```bash
devo                         # 現在のリポジトリで対話型 TUI を起動
devo resume <session-id>
```

## 設定

`devo onboard` が推奨されるセットアップ方法です。provider/model binding を
`config.toml` に書き、API key をユーザースコープの `auth.json` に保存します。

自分の API key とカスタムモデルを手動で設定する場合:

1. `config.toml` で `[model.<slug>]` パラメータ、`[providers.<id>]`、
   `[model_bindings.<id>]` を定義します。
2. シークレットを `DEVO_HOME/auth.json` に置き、`[providers.<id>].credential`
   からその credential id を参照します — API key 自体を `config.toml` に
   書かないでください。
3. エンドポイントのプロトコルに合わせて `invocation_method` を
   `openai_chat_completions`、`openai_responses`、`anthropic_messages` の
   いずれかに設定します。

完全な作業例（カスタムモデルパラメータ + API key）とプロトコルの説明は
[設定](./docs/configuration.ja.md#自分の-api-key-を使う) を参照してください。

## Docs

- [オフラインインストール](./docs/offline-installation.ja.md)
- [設定](./docs/configuration.ja.md)

## よくある質問

### プロジェクトの状態は?

Devo は pre-1.0 で、活発に開発されています。ローカル評価、実験、コントリビューターによる利用には適していますが、
公開 API と設定は今後も変更される可能性があります。

### どのモデルがサポートされていますか?

組み込みモデルメタデータは現在、Qwen、Kimi、MiniMax、GLM、DeepSeek ファミリーをカバーしています。
OpenAI 互換 Chat Completions、OpenAI 互換 Responses、または Anthropic Messages API をサポートするモデルエンドポイントであれば、
provider/model binding を通じて接続できます。

### 自分の API key を使うには?

`devo onboard` を使うか、ユーザースコープの `auth.json` を編集し、
`config.toml` の `[providers.<id>].credential` からその credential id を参照します。
詳細は [設定](./docs/configuration.ja.md#自分の-api-key-を使う) を参照してください。

### Desktop app と TUI/CLI のどちらを使うべきですか?

視覚的なオンボーディング、セッション閲覧、グラフィカルな coding workspace が
必要な場合は Desktop app を使ってください。端末ネイティブな自動化、remote shell
workflow、または既存の command-line setup 内で動く coding agent が必要な場合は
TUI/CLI を使ってください。どちらの surface も同じローカル Devo runtime を対象にしています。

## コントリビュート

プロジェクトはまだ初期段階であり、コントリビューションを歓迎します:

- client/server runtime、provider layer、safety model、TUI に関するアーキテクチャフィードバック。
- ドキュメントと翻訳。
- Provider、model、wire API の対応範囲。
- 検証コマンドと回帰テストを伴う、焦点を絞った修正。

変更について議論するには issue または pull request を開いてください。

## Star 履歴

<a href="https://www.star-history.com/?repos=7df-lab%2Fdevo&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=7df-lab/devo&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=7df-lab/devo&type=date&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=7df-lab/devo&type=date&legend=top-left" />
 </picture>
</a>

## ライセンス

このプロジェクトは [MIT License](./LICENSE) のもとで公開されています。

---

**Devo が役に立った場合は、star をご検討ください。**
