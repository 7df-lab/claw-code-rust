# Git 工作流与上游协作

## 分支策略
- 重要改动在 feature 分支上进行
- 尽可能保持 main 干净并与上游同步
- PR 提交到上游的 main 分支

## 提交信息
- 格式：`type: 简短描述`
- 类型：`feat:` `fix:` `refactor:` `test:` `docs:` `chore:`
- 在提交正文中引用相关 issue

## 提交 PR 前
1. `cargo test` 通过
2. `cargo fmt --all` 格式化
3. `cargo clippy --all` 无警告
4. 验证上游兼容性
5. 写清晰的 PR 描述：做什么/为什么/怎么做

## 上游协作
- 开始重要工作前务必检查上游，避免重复劳动
- 上游合并相关变更时，rebase 或 merge 保持本地同步
- 及时响应维护者对 PR 的反馈

## 开始重要工作前检查
1. 上游是否已实现了这个功能？
2. 是否已有相关的 open issue 或 PR？
3. 是否会与某个 open PR 冲突？
4. 是否有需要更新或添加的测试？
5. 是否需要更新文档？
