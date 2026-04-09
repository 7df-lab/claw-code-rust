<div align="center">

# 🦀 Claw CR

**Open-source coding agent на Rust. Альтернатива ClaudeCode**

[![Status](https://img.shields.io/badge/status-designing-blue?style=flat-square)](https://github.com/)
[![Language](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Origin](https://img.shields.io/badge/origin-Claude_Code_TS-8A2BE2?style=flat-square)](https://docs.anthropic.com/en/docs/claude-code)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/)

[English](./README.md) | [简体中文](./README.zh-CN.md) | [日本語](./README.ja.md) | [한국어](./README.ko.md) | [Español](./README.es.md) | [Français](./README.fr.md) | [Português (Brasil)](./README.pt-BR.md) | [Русский](./README.ru.md)

🚧 Проект на ранней стадии активной разработки — пока не готов к production.

⭐ Поставьте звезду, чтобы следить за проектом

<img 
  src="./docs/assets/screenshot_20260408.png" 
  alt="Project Overview" 
  width="100%"
  style="border-radius: 8px; box-shadow: 0 15px 40px rgba(0,0,0,0.25);object-fit:cover;"
/>

</div>

---

## 📖 Table of Contents

- [Quick Start](#-quick-start)
- [Design Goals](#-design-goals)
- [Contributing](#-contributing)
- [References](#-references)
- [License](#-license)

## 🚀 Quick Start

<!-- ### Install -->

Стабильного релиза пока нет — вы можете собрать проект из исходников по инструкции ниже.

### Build

```bash
git clone https://github.com/claw-cli/claw-code-rust && cd claw-code-rust
cargo build --release

# linux / macos
./target/release/clawcr onboard

# windows
.\target\release\clawcr onboard
```

> [!TIP]
> Убедитесь, что Rust установлен; рекомендуется версия 1.75+ (через https://rustup.rs/).

## FAQ

### Чем это отличается от Claude Code?

По возможностям проект очень похож на Claude Code. Основные отличия:

- 100% open source
- Не привязан к одному провайдеру. ClawCR можно использовать с Claude, OpenAI, z.ai, Qwen, Deepseek или даже с локальными моделями. По мере развития моделей разрыв между ними будет сокращаться, а стоимость снижаться, поэтому независимость от провайдера важна.
- LSP поддерживается из коробки
- TUI уже реализован
- Построен на клиент-серверной архитектуре. Например, core может работать локально на вашем компьютере и при этом управляться удалённо, например из мобильного приложения, а TUI будет лишь одним из возможных клиентов

## 🤝 Contributing

Мы приветствуем вклад в проект. Он находится на ранней стадии проектирования, и помочь можно разными способами:

- **Обратная связь по архитектуре** — изучите дизайн crate'ов и предложите улучшения
- **Обсуждение RFC** — предлагайте новые идеи через issues
- **Документация** — помогайте улучшать или переводить документацию
- **Реализация** — подключайтесь к реализации crate'ов, когда дизайн стабилизируется

Не стесняйтесь открывать issue или отправлять pull request.

## 📄 License

Проект распространяется по [лицензии MIT](./LICENSE).

---

**Если проект оказался полезным, поставьте ему ⭐**
