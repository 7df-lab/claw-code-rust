<div align="center">

# 🦀 Claw CR

**Der Open-Source-Coding-Agent, in Rust gebaut. Alternative zu ClaudeCode**

[![Status](https://img.shields.io/badge/status-designing-blue?style=flat-square)](https://github.com/)
[![Language](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Origin](https://img.shields.io/badge/origin-Claude_Code_TS-8A2BE2?style=flat-square)](https://docs.anthropic.com/en/docs/claude-code)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/)

[English](./README.md) | [简体中文](./README.zh-CN.md) | [日本語](./README.ja.md) | [한국어](./README.ko.md) | [Español](./README.es.md) | [Français](./README.fr.md) | [Deutsch](./README.de.md)

🚧 Frühphasiges Projekt in aktiver Entwicklung - noch nicht produktionsreif.

⭐ Starte uns, um auf dem Laufenden zu bleiben

<img
  src="./docs/assets/screenshot_20260408.png"
  alt="Projektübersicht"
  width="100%"
  style="border-radius: 8px; box-shadow: 0 15px 40px rgba(0,0,0,0.25);object-fit:cover;"
/>

</div>

---

## 📖 Inhaltsverzeichnis

- [Schnellstart](#-schnellstart)
- [FAQ](#-faq)
- [Mitwirken](#-mitwirken)
- [Lizenz](#-lizenz)

## 🚀 Schnellstart

<!-- ### Install -->

Noch keine stabile Veröffentlichung - du kannst das Projekt mit den folgenden Anweisungen aus dem Quellcode bauen.

### Bauen

```bash
git clone https://github.com/claw-cli/claw-code-rust && cd claw-code-rust
cargo build --release

# linux / macos
./target/release/clawcr onboard

# windows
.\target\release\clawcr onboard
```

> [!TIP]
> Stelle sicher, dass Rust installiert ist. Empfohlen wird Version 1.75+ (über https://rustup.rs/).

## FAQ

### Worin unterscheidet sich das von Claude Code?

Es ist Claude Code in Bezug auf die Fähigkeiten sehr ähnlich. Die wichtigsten Unterschiede:

- 100% Open Source
- Nicht an einen Anbieter gebunden. ClawCR kann mit Claude, OpenAI, z.ai, Qwen, Deepseek oder sogar lokalen Modellen verwendet werden. Mit der Weiterentwicklung der Modelle werden sich die Unterschiede verringern und die Preise sinken, daher ist Anbieterunabhängigkeit wichtig.
- LSP-Unterstützung ab Werk
- TUI-Unterstützung ist bereits implementiert.
- Gebaut mit einer Client/Server-Architektur. Zum Beispiel kann der Core lokal auf deinem Rechner laufen, während er remote gesteuert wird, etwa über eine mobile App, wobei die TUI nur ein möglicher Client unter mehreren ist.

## 🤝 Mitwirken

Beiträge sind willkommen. Dieses Projekt befindet sich in einer frühen Designphase, und es gibt viele Möglichkeiten, zu helfen:

- **Architektur-Feedback** - Prüfe das Crate-Design und schlage Verbesserungen vor
- **RFC-Diskussionen** - Schlage neue Ideen über Issues vor
- **Dokumentation** - Hilf dabei, die Dokumentation zu verbessern oder zu übersetzen
- **Implementierung** - Übernimm die Implementierung von Crates, sobald sich die Entwürfe stabilisieren

Bitte eröffne gern ein Issue oder reiche einen Pull Request ein.

## 📄 Lizenz

Dieses Projekt steht unter der [MIT-Lizenz](./LICENSE).

---

**Wenn dir dieses Projekt nützlich ist, gib ihm bitte ein ⭐**
