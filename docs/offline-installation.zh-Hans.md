# 离线安装

[English](./offline-installation.md) | [简体中文](./offline-installation.zh-Hans.md) | [繁體中文](./offline-installation.zh-Hant.md) | [日本語](./offline-installation.ja.md) | [Русский](./offline-installation.ru.md)

许多企业和内网环境无法访问互联网。Devo 安装器支持离线模式，会从安装脚本所在目录读取所有必需资源，
并且不会访问网络。

在一台可以访问互联网的机器上：

1. 下载安装脚本：Linux/macOS 使用 `install.sh`，Windows 使用 `install.ps1`。
2. 下载目标 CPU 和操作系统对应的最新 Devo release asset，例如 `x86_64`
   与 `aarch64`/`arm64`。
3. 下载目标 CPU 和操作系统对应的 `ripgrep` release asset。

把这些文件放在安装脚本旁边。

Linux / macOS:

```bash
sh ./install.sh --offline
```

Windows:

```powershell
.\install.ps1 -Offline
```
