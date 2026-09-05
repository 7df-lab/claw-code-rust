# オフラインインストール

[English](./offline-installation.md) | [简体中文](./offline-installation.zh-Hans.md) | [繁體中文](./offline-installation.zh-Hant.md) | [日本語](./offline-installation.ja.md) | [Русский](./offline-installation.ru.md)

多くのエンタープライズ環境やイントラネット環境ではインターネットにアクセスできません。
Devo のインストーラーはオフラインモードをサポートしており、必要なすべてのアセットをインストーラースクリプトと同じディレクトリから読み込み、ネットワークには接続しません。

インターネットにアクセスできるマシンで:

1. インストーラースクリプトをダウンロードします。Linux/macOS は `install.sh`、Windows は `install.ps1` です。
2. 対象 CPU と OS 向けの最新 Devo release asset をダウンロードします。例: `x86_64`
   と `aarch64`/`arm64`。
3. 対象 CPU と OS に合う `ripgrep` release asset をダウンロードします。

これらのファイルをインストーラースクリプトの隣に置きます。

Linux / macOS:

```bash
sh ./install.sh --offline
```

Windows:

```powershell
.\install.ps1 -Offline
```
