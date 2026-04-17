# Rust 编码与测试规范

## 编码规范
- 优先使用带内联变量的 `format!()`
- 折叠嵌套 `if` 语句
- 适用时用方法引用替代闭包
- 避免模糊的 `bool`/`Option` 参数，用枚举或命名方法
- `match` 表达式必须穷尽
- 新 trait 必须加文档注释
- 模块 500 行以内，约 800 行时拆分
- 不中断 `cargo test` 或 `just fix`（Rust 并行可能导致临时锁）

## 测试规范
- 用 `pretty_assertions::assert_eq` 获得更清晰 diff
- 比较完整对象，不逐字段比较
- 平台感知路径：按需 `#[cfg(windows)]` / `#[cfg(unix)]`
- 测试中不修改环境变量，显式传入值
