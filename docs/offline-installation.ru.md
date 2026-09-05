# Офлайн-установка

[English](./offline-installation.md) | [简体中文](./offline-installation.zh-Hans.md) | [繁體中文](./offline-installation.zh-Hant.md) | [日本語](./offline-installation.ja.md) | [Русский](./offline-installation.ru.md)

Многие корпоративные и intranet-среды не имеют доступа к интернету. Установщики
Devo поддерживают офлайн-режим: они читают все необходимые assets из того же
каталога, что и скрипт установщика, и не обращаются к сети.

На машине с доступом к интернету:

1. Скачайте скрипт установщика: `install.sh` для Linux/macOS или `install.ps1`
   для Windows.
2. Скачайте последний Devo release asset для целевой CPU и OS, например
   `x86_64` или `aarch64`/`arm64`.
3. Скачайте соответствующий `ripgrep` release asset для целевой CPU и OS.

Положите эти файлы рядом со скриптом установщика.

Linux / macOS:

```bash
sh ./install.sh --offline
```

Windows:

```powershell
.\install.ps1 -Offline
```
