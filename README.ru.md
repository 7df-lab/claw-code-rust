<div align="center">

<img src="./.github/assets/devo-readme-brand.svg" alt="Devo desktop coding agent app icon and wordmark" width="360" />

</div>

<div align="center">

**Devo - open-source coding agent с Desktop app, terminal TUI/CLI и модельно-нейтральным Rust runtime для приватных, корпоративных и OpenAI-совместимых модельных сред. Подключайте DeepSeek, Qwen, Kimi, Anthropic-совместимые API, локальные шлюзы или собственные endpoint моделей.**

[![Stars](https://img.shields.io/github/stars/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/stargazers)
[![Language](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/7df-lab/devo/pulls)
[![CI](https://img.shields.io/github/actions/workflow/status/7df-lab/devo/ci.yml?branch=main&style=flat-square)](https://github.com/7df-lab/devo/actions)
[![Release](https://img.shields.io/github/v/release/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/releases)

[English](./README.md) | [简体中文](./README.zh-Hans.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | [Русский](./README.ru.md)

[Почему Devo](#почему-devo) · [Скриншоты](#скриншоты) · [Возможности](#возможности) · [Проверенные модели](#проверенные-модели) · [Проверенные платформы](#проверенные-платформы) · [Установка](#установка) · [Быстрый старт](#быстрый-старт) · [Документация](#docs)

</div>

---

## Скриншоты

<p align="center">
  <img width="100%" alt="Devo desktop coding agent app показывает беседу по репозиторию, боковую панель проекта и управление моделью" src="./.github/assets/devo-desktop-coding-agent-screenshot.png" />
</p>

<p align="center">
  <img width="100%" alt="Devo terminal TUI coding agent работает в локальном репозитории и показывает модель, контекст и token status" src="./.github/assets/devo-terminal-tui-coding-agent-screenshot.png" />
</p>

## Почему Devo

Devo предназначен для команд, которым нужен coding agent вне единой hosted
экосистемы моделей. Он оставляет Desktop experience, terminal workflow, выбор
модели, поведение runtime и выполнение в workspace под вашим контролем.

- **Подключайте свою модель** - Через provider/model bindings можно подключать
  OpenAI-compatible Chat Completions, OpenAI-compatible Responses, Anthropic
  Messages, DeepSeek, Qwen, Kimi или приватные model gateways.
- **Подходит для приватных и intranet-сред** - Devo запускается как единый
  локальный Rust binary, поддерживает offline installation paths и может
  указывать на внутренние endpoint без зависимости от hosted agent service.
- **Один agent для Desktop и terminal** - Используйте Desktop app для
  визуального onboarding и повседневного coding, либо CLI/TUI для
  terminal-native automation, remote shell и scriptable workflows.
- **Расширяемый agent runtime** - MCP servers, reusable skills, аудируемые
  сессии, permissions и multi-agent flows являются возможностями runtime, а не
  одноразовыми prompt.

## Возможности

- **Модельно-нейтральный provider runtime** - Используйте provider/model bindings
  для OpenAI-совместимых, Anthropic-совместимых, DeepSeek, Qwen, Kimi, GLM,
  MiniMax, Xiaomi MiMo, OpenRouter или локальных endpoint.
- **Поддержка MCP** - Подключайте внешние инструменты и контекст через серверы
  [Model Context Protocol](https://modelcontextprotocol.io/). Управляйте через
  CLI: `devo mcp add|list|enable|disable|remove` (см.
  [Конфигурацию](./docs/configuration.ru.md#mcp-серверы)).
- **Поддержка Skill** - Упаковывайте повторяемые workflow, инструкции, скрипты
  и справочные материалы как переиспользуемые
  [Agent Skills](https://agentskills.io/).
- **Поддержка долгих задач** - Позвольте Devo автоматически управлять контекстом
  в многошаговой работе, чтобы не терять ход задачи по мере ее роста.
- **Поддержка нескольких агентов** - Разделяйте работу между специализированными
  агентами, сохраняя координацию видимой в сессии.
- **Plan Mode** - Разбивайте крупные задачи на понятные многошаговые планы до
  начала реализации.
- **Параллельные вызовы инструментов** - Запускайте несколько независимых
  инструментов параллельно, чтобы модели меньше ждали и быстрее продвигались.
- **Выполнение инструментов с разрешениями** - Проверяйте чувствительные вызовы
  инструментов до того, как они затронут рабочую область.
- **Аудируемые сессии** - Храните вывод модели, вызовы инструментов, approvals,
  расход token и историю сессии в виде, пригодном для проверки и возобновления.
- **Видимость стоимости и контекста** - Показывайте input/output token,
  cached token и использование context window там, где провайдеры это раскрывают.
- **Легковесный Rust runtime** - Построен на Rust, с малым расходом памяти и
  компактным локальным runtime.
- **Встроенный семантический поиск по коду (MCP)** - Опциональный bundled MCP
  сервер (`code_search` / `devo-code-search-mcp`), **по умолчанию выключен**.
  Запускает локальную CPU-модель эмбеддингов и сочетает dense retrieval с BM25,
  сокращая контекст поиска по сравнению с агентами только на grep/find. Включите
  через `devo mcp enable code_search` или TUI `/mcps`.

## Проверенные модели

<p>
  <img alt="DeepSeek v4 Flash / Pro" src="https://img.shields.io/badge/DeepSeek-v4%20Flash%20%2F%20Pro-4D6BFE?style=flat-square&logo=deepseek&logoColor=white" />
  <img alt="GLM 5.2" src="https://img.shields.io/badge/GLM-5.2-7856FF?style=flat-square&logo=huggingfacei&logoColor=white" />
  <img alt="MiniMax M3" src="https://img.shields.io/badge/MiniMax-M3-0B5FFF?style=flat-square&logo=minimax&logoColor=white" />
  <img alt="Qwen3 Coder Next" src="https://img.shields.io/badge/Qwen3-Coder%20Next-615CED?style=flat-square&logo=qwen&logoColor=white" />
  <img alt="Kimi K3" src="https://img.shields.io/badge/Kimi-K3-111111?style=flat-square&logo=moonshotai&logoColor=white" />
</p>

Встроенный каталог моделей Devo содержит проверенные определения моделей для
Qwen, Kimi, MiniMax, GLM и DeepSeek. Endpoint поставщиков остаются настраиваемыми
через provider/model bindings.

## Проверенные платформы

<p>
  <img alt="macOS проверен" src="https://img.shields.io/badge/macOS-tested-000000?style=flat-square&logo=apple&logoColor=white" />
  <img alt="Linux проверен" src="https://img.shields.io/badge/Linux-tested-FCC624?style=flat-square&logo=linux&logoColor=000000" />
  <img alt="Windows проверен" src="https://img.shields.io/badge/Windows-tested-0078D4?style=flat-square&logo=windows&logoColor=white" />
</p>

Devo протестирован на macOS, Linux, Windows и Kylin OS.

### Для китайских корпоративных пользователей

<p>
  <img alt="Kylin OS проверен" src="https://img.shields.io/badge/Kylin%20OS-tested-1E88E5?style=flat-square" />
  <img alt="Поддержка HarmonyOS в планах" src="https://img.shields.io/badge/HarmonyOS-on%20the%20road-111111?style=flat-square&logo=harmonyos&logoColor=white" />
</p>

Поддержка Kylin OS выделена отдельно, потому что отечественные операционные системы
часто являются реальным требованием при внедрении в китайских корпоративных средах.
Поддержка HarmonyOS находится в roadmap; мы приветствуем вклад участников с
устройствами HarmonyOS, которые смогут собрать, протестировать и опубликовать
релизы для этой платформы.

## Установка

Devo можно установить в двух формах. Выберите Desktop app для графического
coding agent workspace, terminal-native TUI/CLI для shell-first разработки или
установите оба варианта на одной машине.

### Вариант 1: Desktop App

Начните здесь, если хотите использовать графический интерфейс Devo. Скачайте
последний Devo Desktop package со страницы
[GitHub Releases](https://github.com/7df-lab/devo/releases/latest), затем
выберите asset для вашей операционной системы и архитектуры:

- **macOS** - скачайте `.dmg` или `.zip` asset вида
  `devo-desktop-...-mac-...`.
- **Windows** - скачайте `.exe` asset вида `devo-desktop-...-windows-...`.
- **Linux** - скачайте `.AppImage`, `.deb` или `.rpm` asset вида
  `devo-desktop-...-linux-...`.

**Если macOS сообщает, что `Devo.app` повреждено и не может быть открыто, это
ожидаемо.** Текущие macOS Desktop builds не подписаны, поэтому после установки
выполните следующую команду, чтобы macOS могла запустить приложение:

```bash
sudo xattr -dr com.apple.quarantine /Applications/Devo.app
```

### Вариант 2: TUI / CLI

Установите terminal-native команду `devo`, если предпочитаете TUI, хотите
shell automation или хотите использовать Devo вместе с Desktop app.

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
```

Windows:

```powershell
irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

Онлайн-установщик размещает `devo` в Devo home directory, устанавливает
вспомогательный `rg` sidecar для быстрого поиска по репозиторию и поддерживает
дополнительную настройку локальной модели, которую использует `code_search`.

<details>
<summary>Необязательно: предварительно установить локальную модель <code>code_search</code></summary>

Используйте это только если хотите скачать модель Hugging Face во время установки.

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh -s -- --install-code-search-model
```

Windows:

```powershell
$env:DEVO_INSTALL_CODE_SEARCH_MODEL = "1"; irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

</details>

Обновление существующей установки до последнего release:

```bash
devo upgrade
```

Команда обновления запускает тот же установщик для текущей платформы, а
установщик выводит переход версии, например `Version: v0.1.12 -> v0.1.15`.

Для intranet-сред или установки без доступа к сети см.
[Офлайн-установку](./docs/offline-installation.ru.md).

## Быстрый старт

Настройте provider, откройте репозиторий и запустите TUI:

```bash
cd /path/to/your/repo
devo onboard
```

Полезные команды:

```bash
devo                         # запустить интерактивный TUI в текущем репозитории
devo resume <session-id>
```

## Конфигурация

`devo onboard` - рекомендуемый путь настройки. Он записывает provider и model
bindings в `config.toml` и сохраняет API key в пользовательском `auth.json`.

Чтобы вручную подключить свой ключ и кастомную модель:

1. Определите параметры `[model.<slug>]`, `[providers.<id>]` и
   `[model_bindings.<id>]` в `config.toml`.
2. Положите секрет в `DEVO_HOME/auth.json` и ссылайтесь на этот credential id из
   `[providers.<id>].credential` — не пишите сам API key в `config.toml`.
3. Установите `invocation_method` в соответствии с протоколом endpoint:
   `openai_chat_completions`, `openai_responses` или `anthropic_messages`.

Полный пример (параметры кастомной модели + API key) и описание протоколов:
[Конфигурация](./docs/configuration.ru.md#свой-api-key).

## Docs

- [Офлайн-установка](./docs/offline-installation.ru.md)
- [Конфигурация](./docs/configuration.ru.md)

## Часто задаваемые вопросы

### Каков статус проекта?

Devo находится на стадии pre-1.0 и активно развивается. Он готов для локальной
оценки, экспериментов и использования участниками проекта; публичные API и
конфигурация еще могут меняться.

### Какие модели поддерживаются?

Встроенные метаданные моделей сейчас покрывают семейства Qwen, Kimi, MiniMax,
GLM и DeepSeek. Любой endpoint модели, который поддерживает OpenAI-compatible
Chat Completions, OpenAI-compatible Responses или Anthropic Messages API, можно
подключить через provider/model bindings.

### Как подключить свой API key?

Используйте `devo onboard` или отредактируйте пользовательский `auth.json` и
укажите этот credential id в `[providers.<id>].credential` в `config.toml`. См.
[Конфигурацию](./docs/configuration.ru.md#свой-api-key).

### Что выбрать: Desktop app или TUI/CLI?

Используйте Desktop app, если вам нужны visual onboarding, просмотр сессий и
графический coding workspace. Используйте TUI/CLI, если вам нужны
terminal-native automation, remote shell workflows или coding agent внутри
существующей command-line setup. Оба интерфейса работают с одним локальным
Devo runtime.

## Участие в разработке

Вклад приветствуется, пока проект остается ранним:

- Архитектурная обратная связь по client/server runtime, provider layer, safety
  model и TUI.
- Документация и переводы.
- Покрытие Provider, model и wire API.
- Точечные исправления с командами проверки и регрессионными тестами.

Откройте issue или pull request, чтобы обсудить изменения.

## История звезд

<a href="https://www.star-history.com/?repos=7df-lab%2Fdevo&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=7df-lab/devo&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=7df-lab/devo&type=date&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=7df-lab/devo&type=date&legend=top-left" />
 </picture>
</a>

## Лицензия

Проект распространяется по [MIT License](./LICENSE).

---

**Если Devo оказался полезен, пожалуйста, поставьте ему star.**
