# 離線安裝

[English](./offline-installation.md) | [简体中文](./offline-installation.zh-Hans.md) | [繁體中文](./offline-installation.zh-Hant.md) | [日本語](./offline-installation.ja.md) | [Русский](./offline-installation.ru.md)

許多企業和內網環境無法存取網際網路。Devo 安裝器支援離線模式，會從安裝腳本所在目錄讀取所有必需資源，
並且不會存取網路。

在一台可以存取網際網路的機器上：

1. 下載安裝腳本：Linux/macOS 使用 `install.sh`，Windows 使用 `install.ps1`。
2. 下載目標 CPU 和作業系統對應的最新 Devo release asset，例如 `x86_64`
   與 `aarch64`/`arm64`。
3. 下載目標 CPU 和作業系統對應的 `ripgrep` release asset。

把這些檔案放在安裝腳本旁邊。

Linux / macOS:

```bash
sh ./install.sh --offline
```

Windows:

```powershell
.\install.ps1 -Offline
```
