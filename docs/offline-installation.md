# Offline Installation

[English](./offline-installation.md) | [简体中文](./offline-installation.zh-Hans.md) | [繁體中文](./offline-installation.zh-Hant.md) | [日本語](./offline-installation.ja.md) | [Русский](./offline-installation.ru.md)

Many enterprise and intranet environments do not have internet access. Devo's
installers support an offline mode that reads all required assets from the same
directory as the installer script and does not contact the network.

On a machine with internet access:

1. Download the installer script: `install.sh` for Linux/macOS or `install.ps1`
   for Windows.
2. Download the latest Devo release asset for the target CPU and OS, for example
   `x86_64` vs `aarch64`/`arm64`.
3. Download the matching `ripgrep` release asset for the target CPU and OS.

Place these files next to the installer script.

Linux / macOS:

```bash
sh ./install.sh --offline
```

Windows:

```powershell
.\install.ps1 -Offline
```
