<div align="center">

# 🦀 Claw CR

**O agente de codificação open source, construído em Rust. Uma alternativa ao Claude Code**

[![Status](https://img.shields.io/badge/status-designing-blue?style=flat-square)](https://github.com/)
[![Language](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Origin](https://img.shields.io/badge/origin-Claude_Code_TS-8A2BE2?style=flat-square)](https://docs.anthropic.com/en/docs/claude-code)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/)

[English](./README.md) | [简体中文](./README.zh-CN.md) | [日本語](./README.ja.md) | [한국어](./README.ko.md) | [Español](./README.es.md) | [Français](./README.fr.md) | [Português do Brasil](./README.pt-BR.md)

🚧 Projeto em estágio inicial e em desenvolvimento ativo. Ainda não está pronto para produção.

⭐ Dê uma estrela para acompanhar

<img
  src="./docs/assets/screenshot_20260408.png"
  alt="Visão geral do projeto"
  width="100%"
  style="border-radius: 8px; box-shadow: 0 15px 40px rgba(0,0,0,0.25);object-fit:cover;"
/>

</div>

---

## 📖 Índice

- [Início Rápido](#-início-rápido)
- [FAQ](#-faq)
- [Contribuindo](#-contribuindo)
- [Licença](#-licença)

## 🚀 Início Rápido

<!-- ### Instalação -->

Ainda não há uma versão estável. Você pode compilar o projeto a partir do código-fonte usando as instruções abaixo.

### Compilar

```bash
git clone https://github.com/claw-cli/claw-code-rust && cd claw-code-rust
cargo build --release

# linux / macos
./target/release/clawcr onboard

# windows
.\target\release\clawcr onboard
```

> [!TIP]
> Verifique se o Rust está instalado. Recomenda-se a versão 1.75+ (via https://rustup.rs/).

## FAQ

### Em que isso é diferente do Claude Code?

É muito semelhante ao Claude Code em termos de capacidade. As principais diferenças são:

- 100% open source
- Não depende de um único provedor. O ClawCR pode ser usado com Claude, OpenAI, z.ai, Qwen, Deepseek ou até modelos locais. À medida que os modelos evoluem, as diferenças entre eles tendem a diminuir e os preços também devem cair, então ser independente de provedor é importante.
- Suporte a LSP pronto para uso
- O suporte a TUI já está implementado
- Construído com arquitetura cliente/servidor. Por exemplo, o core pode rodar localmente na sua máquina enquanto é controlado remotamente, como por um app móvel, com a TUI sendo apenas um entre vários clientes possíveis

## 🤝 Contribuindo

Contribuições são bem-vindas. Este projeto ainda está na fase inicial de design, e há muitas formas de ajudar:

- **Feedback de arquitetura** - Revise o design dos crates e sugira melhorias
- **Discussões de RFC** - Proponha novas ideias por meio de issues
- **Documentação** - Ajude a melhorar ou traduzir a documentação
- **Implementação** - Assuma a implementação dos crates quando os designs estiverem mais estáveis

Sinta-se à vontade para abrir uma issue ou enviar um pull request.

## 📄 Licença

Este projeto está licenciado sob a [Licença MIT](./LICENSE).

---

**Se você achar este projeto útil, considere dar uma ⭐**
