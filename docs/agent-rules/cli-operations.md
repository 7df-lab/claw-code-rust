# CLI 操作、通知系统与调试

## PowerShell 注意事项
- `&&` 不可用，用 `;` 连接命令
- `curl` 是 PowerShell 别名，需要用 `curl.exe`
- 文件读写只在项目目录内进行，不用系统 `%TEMP%`
- `Out-File` 写系统临时目录会被安全工具拦截，避免使用

## Git 操作
- 优先用 Git MCP 工具（add、commit、status、diff、log、branch）
- push/pull 等 MCP 不支持的才用命令行
- 有未提交更改时先 `git stash` 再 pull

## 通知系统
- 通知文件：`notifications/github-meta.json`（元数据）+ `github-activity.jsonl`（事件日志）
- Actions 每 30 分钟采集：上游 commits、PR 活动、issue 更新、评论
- Agent 消费行为：分析含义 → 汇报用户 → 社交类只建议不行动 → 技术类自主处理
- 读取后更新 `last_read_timestamp`

## 调试方法论
- 遇到 bug 先复现，再定位，最后修复
- GitHub Actions 调试：将日志写入仓库文件（会被提交推送），运行后读取
- API 调试：先在本地用 `curl.exe` 或 `mcp_fetch_fetch` 验证端点可用
- 变量展开问题：shell heredoc 不可靠，用 `jq -n` 构建 JSON
- 权限问题：`GITHUB_TOKEN` 只能访问当前仓库，上游公开仓库用 `curl` 无认证 API
