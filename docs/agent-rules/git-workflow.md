# Git 工作流与上游协作

## 分支策略
- `fork/main`：自己的维护分支，改什么都可以，不care上游
- `fork/feat/xxx`：专门给上游提 PR 的分支，只放要提交的改动
- **不要在 main 上直接做要给上游的改动**

## 提 PR 流程
1. `git fetch upstream` 拉取上游最新
2. 从 `upstream/main` 创建干净的功能分支：`git checkout -b feat/xxx upstream/main`
3. 在这个分支上只做要给上游的改动
4. 提交并 push 到自己的 fork
5. 从这个分支提 PR 到上游

## 为什么要这样做
- 避免 fork main 和上游 main 分叉
- PR diff 干净，只显示真正的功能改动
- 不把 Agent 专用文件混进去

## 提交信息
- 格式：`type: 简短描述`
- 类型：`feat:` `fix:` `refactor:` `test:` `docs:` `chore:`
- 在提交正文中引用相关 issue

## 提交频率
- **`fork/main`（个人维护分支）**：可以频繁提交，每次改动随时 commit，便于回滚和追踪历史
- **`fork/feat/xxx`（给上游提 PR 的分支）**：提 PR 前用 `git rebase -i` 整理成少量干净的 commit
- 原则：宁可多提交，也不要丢失工作进度

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
