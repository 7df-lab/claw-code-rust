<div align="center">

# 🦀 Claw CR

**以 Rust 打造的開源程式碼代理。Claude Code 的替代方案。**

[![狀態](https://img.shields.io/badge/status-designing-blue?style=flat-square)](https://github.com/)
[![語言](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![來源](https://img.shields.io/badge/origin-Claude_Code_TS-8A2BE2?style=flat-square)](https://docs.anthropic.com/en/docs/claude-code)
[![授權](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![歡迎 PR](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/)

[English](./README.md) | [繁體中文](./README.zh-TW.md) | [简体中文](./README.zh-CN.md) | [日本語](./README.ja.md) | [한국어](./README.ko.md) | [Español](./README.es.md) | [Français](./README.fr.md) | [Deutsch](./README.de.md) | [Português do Brasil](./README.pt-BR.md) | [Русский](./README.ru.md)

🚧 這是一個仍處於早期、持續開發中的專案，尚未達到可正式上線的程度。

⭐ 歡迎幫我們點個星星

<img
  src="./docs/assets/screenshot_20260408.png"
  alt="專案概覽"
  width="100%"
  style="border-radius: 8px; box-shadow: 0 15px 40px rgba(0,0,0,0.25);object-fit:cover;"
/>

</div>

---

## 📖 目錄

- [快速開始](#-快速開始)
- [常見問題](#-常見問題)
- [參與貢獻](#-參與貢獻)
- [授權](#-授權)

## 🚀 快速開始

目前還沒有穩定版本，你可以依照下列步驟從原始碼建置專案。

### 建置

```bash
git clone https://github.com/claw-cli/claw-code-rust && cd claw-code-rust
cargo build --release

# linux / macos
./target/release/clawcr onboard

# windows
.\target\release\clawcr onboard
```

> [!TIP]
> 請先確認已安裝 Rust，建議使用 1.75+（可透過 https://rustup.rs/ 安裝）。

## 常見問題

### 這和 Claude Code 有什麼不同？

在能力上，它和 Claude Code 非常接近。主要差異如下：

- 100% 開源
- 不綁定任何單一供應商。ClawCR 可以搭配 Claude、OpenAI、z.ai、Qwen、Deepseek，甚至本地模型使用。隨著模型持續演進，差距會縮小，價格也會下降，因此保持 provider 無關非常重要。
- 開箱即用的 LSP 支援
- 已經實作 TUI 支援
- 採用 client/server 架構。例如，core 可以在你的本機執行，同時由遠端控制，例如透過手機 App 操作，而 TUI 只是眾多 client 之一。

## 🤝 參與貢獻

歡迎貢獻！這個專案仍處於早期設計階段，有很多方式可以參與：

- **架構回饋** - 審閱 crate 設計並提出改善建議
- **RFC 討論** - 透過 issue 提出新想法
- **文件** - 協助改善或翻譯文件
- **實作** - 等設計穩定後，協助推進 crate 實作

歡迎直接開 issue 或提交 pull request。

## 📄 授權

本專案採用 [MIT 授權](./LICENSE)。

---

**如果你覺得這個專案有幫助，歡迎給它一個 ⭐**
