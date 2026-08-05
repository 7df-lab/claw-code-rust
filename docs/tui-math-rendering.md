# TUI Markdown 数学公式渲染

TUI 对 Markdown 的 `$...$` 和 `$$...$$` 数学公式使用 `term-maths` 渲染为终端可读的 Unicode 字符画。

- 行内公式会使用 Unicode 上标、下标和数学符号，例如 `x^2` 显示为 `x²`。
- 块级公式会保留分数、根号、矩阵和求和的二维布局，并作为结构化的不可换行行渲染。
- 流式输出会保留未闭合的 `$$` 数学块，直到闭合分隔符到达后再提交到历史记录。
- 公式源文本仍会保存在最终的 Markdown 历史单元中，因此终端尺寸变化后可以重新布局普通文本而不破坏公式结构。

仓库使用 `ratatui` 的 patched 0.29 分支，该分支固定 `unicode-width 0.2.1`；`term-maths 1.0.0` 要求 `unicode-width 0.2.2`。为保持现有 TUI 依赖不变，`vendor/term-maths` 保留 crates.io 版本源码，仅将其兼容性依赖约束调整为 `0.2.1`，并通过 workspace 的 `[patch.crates-io]` 使用。
