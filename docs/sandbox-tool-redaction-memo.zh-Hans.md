# Devo 工具隔离与敏感信息脱敏备忘录

## 目的

本文记录当前 Devo 的工具执行边界、OS sandbox 覆盖范围，以及工具输出中的敏感信息脱敏现状。后续修复应以本文的审计结论为基线，避免把应用层权限、OS 子进程 sandbox 和输出脱敏混成同一条链路。

## 当前结论

当前 Devo 只有外部子进程执行路径稳定进入 OS sandbox。原生文件工具、搜索工具、网络工具和技能加载工具大多直接在 Devo 主进程中运行，因此不会自动受到 macOS `sandbox-exec` 或 Linux Landlock/bwrap 的约束。

仓库中已经存在 `devo-safety` 的正则 secret detector 和 `SecretRedactor`，但当前没有发现任何 runtime 调用点把它接入 `ToolResult`、模型请求、协议事件、TUI 展示或日志 layer。`redact_secrets_in_logs` 目前只作为配置字段和初始化日志字段存在，没有实际的日志脱敏实现。

## 一、OS sandbox 覆盖范围

### 已进入 OS sandbox 的路径

- `shell_command` / `bash`：通过普通 pipe 或 PTY 启动 shell。
- `exec_command`：通过 unified exec process 启动子进程。
- unified exec 的 PTY process。
- `write_stdin`：本身不创建进程，只操作已经启动的进程，因此继承启动时的 sandbox。

关键入口：

- `crates/core/src/tools/shell_exec.rs`
- `crates/core/src/tools/unified_exec/process.rs`
- `crates/utils/process/src/pty/pipe.rs`
- `crates/sandbox/src/wrap.rs`

macOS 当前使用父进程构造的 `sandbox-exec -p <SBPL>` wrapper。Linux 使用 `devo-linux-sandbox`、`bwrap` 和 pipe 路径中的 Landlock/nono 组合。profile 在父进程中解析，child 的 `pre_exec` 只接收已经解析的 enforcement plan。

### 未进入 OS sandbox 的工具

以下工具在当前实现中没有把 `sandbox_profile` 传入统一的 OS sandbox spawn 边界：

- `read`
- `write`
- `edit`
- `apply_patch`
- `find`
- `grep`
- `code_search`
- `webfetch`
- `web_search`
- `skill`
- MCP tools

具体情况：

1. `read`、`write`、`edit` 使用 `tokio::fs` 或普通文件 API，在 Devo 主进程内读写文件。
2. `apply_patch` 使用内部 patch executor 直接修改文件。
3. `find` 和 `grep` 会启动 `rg`，但 `crates/core/src/tools/handlers/ripgrep.rs` 中的 `run_rg` 没有应用 `ToolContext.sandbox_profile`，因此这个 `rg` 子进程是普通未包装进程。
4. `code_search` 通过 `CodeSearchService` 构建/查询索引，主要在 Devo 进程内访问 workspace。
5. `webfetch` 和 `web_search` 在 Devo 进程内使用 HTTP client。它们可以使用 proxy 配置，但 `restrict_network` 不会自动变成 macOS Seatbelt 网络 deny。
6. `skill` 直接递归查找并读取 `SKILL.md` 及其相邻文件。
7. MCP tool 的调用交给 `McpManager`；Devo 当前没有在 MCP tool 调用边界增加统一的 OS sandbox。MCP server 是否隔离取决于 server 自己的启动实现。

`plan`、`update_goal`、`question`、`ToolSearch`、agent 协调等工具不直接操作 workspace 或网络，当前没有 OS sandbox 并不构成同类文件/网络隔离缺口。

## 二、应用层权限与 OS sandbox 的区别

`ToolExecutionMode::ReadOnly`、permission router、capability tags 和用户确认属于 Devo 应用层权限模型，不等于 macOS App Sandbox 或 Seatbelt。

当前 router 可以：

- 在工具执行前进行 permission check。
- 根据审批结果允许或拒绝工具调用。
- 对 shell family 的 `SANDBOX_DENIED` 做一次 `off` 重试。
- 当 profile 有 deny-read 路径时，禁止静默关闭 sandbox。

但这些逻辑不会自动把原生 `read`、`write`、`find`、`code_search` 等工具放进 OS sandbox。应用层授权和 OS capability 必须分别审计。

## 三、DEVO_HOME 与 auth.json

当前 `crates/sandbox/src/paths.rs` 的 `essential_writable_paths` 把整个 `DEVO_HOME` 加入 writable roots。当前没有发现对：

```text
$DEVO_HOME/auth.json
```

单独生成 deny-read 或 deny-write 规则的实现。

因此目前实际语义是：

```text
DEVO_HOME       可写
DEVO_HOME/auth.json 也随目录可写
```

这不符合“DEVO_HOME 默认可读写，但 auth.json 不应被工具读写”的目标。该目标需要同时覆盖：

- shell / PTY / pipe 子进程；
- 原生 read/write/edit/apply_patch；
- find/grep/code_search 索引；
- skill 文件加载；
- 可能访问本地文件的 MCP tool；
- 允许用户显式查看或更新 auth.json 的配置流程。

## 四、SecretRedactor 当前状态

### 已存在的基础设施

`crates/safety/src/lib.rs` 已经定义：

- `REDACTED_SECRET_PLACEHOLDER = "[REDACTED_SECRET]"`。
- `SecretMatchConfidence`。
- `SecretDetector` 和 `SecretDetectorRegistry`。
- `RegexSecretDetector`。
- `InMemorySecretDetectorRegistry::with_default_detectors()`。
- `SecretRedactor::redact()`。
- `RedactionResult` 和 `RedactionReport`。

默认 detector 当前包括：

- OpenAI 风格 `sk-...` key。
- AWS access key id。
- Bearer token。
- `api_key`、`token`、`secret`、`password` 赋值形式。

这些规则在 `devo-safety` 单元测试中能够把匹配内容替换为 `[REDACTED_SECRET]`。

### 当前缺失的 wire

对整个仓库进行 `SecretRedactor`、`RegexSecretDetector`、`RedactionResult` 和 `redact(` 的调用点搜索，目前只找到定义和 `devo-safety` 测试，没有找到 runtime 接线。

当前 `ToolResult` 的主要路径是：

```text
ToolHandler::handle
    ↓
ToolResult
    ↓
router / query loop
    ↓
QueryEvent::ToolResult
    ↓
RequestContent::ToolResult / ContentBlock::ToolResult
    ↓
provider request、protocol event、ACP/TUI projection
```

在这些边界上目前没有统一执行 `SecretRedactor`：

- `crates/core/src/tools/contracts.rs` 的 `ToolResult`。
- `crates/core/src/tools/router.rs` 的工具返回处理。
- `crates/core/src/query/mod.rs` 的 tool result message 构造。
- `crates/core/src/query/event.rs` 的 `QueryEvent::ToolResult`。
- `crates/protocol/src/event.rs` 的 `ToolResultPayload`。
- `crates/protocol/src/acp/event_to_update.rs` 的 `raw_output`、`content` 投影。
- TUI 的 tool result 和 tool output delta 事件。

因此当前存在的风险是：一个工具只要返回包含 API key 的文本或 JSON，该值可能继续进入：

- 模型下一轮的 tool result message。
- ACP/server protocol payload。
- TUI transcript、raw output 或 tool cell。
- durable history 或诊断记录。
- 日志中的结构化字段或错误文本。

### 日志配置也尚未真正接线

`LoggingConfig.redact_secrets_in_logs` 的默认值是 `true`，但 `crates/core/src/logging.rs` 当前只是把该布尔值写入 `tracing initialized` 事件，没有安装 redaction tracing layer，也没有调用 `SecretRedactor` 处理日志字段。

所以需要区分：

```text
配置字段存在       是
SecretRedactor 类型存在  是
工具输出 wire       否
模型请求 wire        否
协议/TUI wire        否
日志实际过滤        未发现
```

## 五、建议的修复架构

### 1. 统一 policy context

为每次 tool invocation 构造不可变的 `ToolSecurityContext`，至少包含：

- workspace root。
- sandbox profile。
- readable roots。
- writable roots。
- deny paths。
- network policy。
- `DEVO_HOME` 和 `auth.json` 的特殊规则。
- active `SecretRedactor`。

所有本地文件、搜索、网络和子进程工具都必须从这个 context 获取策略，而不是各自决定是否检查。

### 2. 先修复文件/搜索访问边界

优先级建议：

1. 给 `read`、`write`、`edit`、`apply_patch` 增加统一路径 capability check。
2. 让 `find`、`grep`、`code_search` 使用同一套 readable roots 和 deny paths。
3. 修复 `run_rg`：如果保留外部 `rg`，必须经过统一 child spawn wrapper；更理想的是让搜索服务接受显式 filesystem policy。
4. 让 `skill` 只能读取允许的 skill roots，不能通过 workspace 递归搜索绕过 deny。
5. 对 MCP tool 明确声明 filesystem/network capability；没有声明的 tool 默认 deny 或 ask。

### 3. 处理 auth.json

不要只依赖 shell profile。应将 auth 文件定义成专门的 `SecretPath`：

- 默认禁止工具读取。
- 默认禁止工具写入。
- 日常 provider resolution 在受控的 config/auth 组件内完成。
- 用户显式修改 credential 时走专用配置流程。
- 对返回错误、diagnostic 和日志继续做 secret redaction。

## 六、SecretRedactor 的接线方案

建议设置两个明确的边界：

### 模型可见边界

在 tool result 进入下一轮模型请求之前执行 redaction：

```text
ToolResult
    → normalize text / JSON
    → SecretRedactor
    → model-visible ToolResult
```

必须覆盖 `Text`、`Json` 和 `Mixed` 三种 `ToolResultContent`，不能只处理字符串 variant。JSON 应递归处理所有 string value，同时保留 JSON 结构。

### 外部可见边界

在协议事件、ACP update、TUI transcript 和 durable history 写入前，使用同一个 redacted representation，避免模型看不到但 UI 或日志仍显示原文。

建议明确区分：

```text
canonical_internal_result  原始值，仅短生命周期、最小范围保留
model_visible_result       脱敏后
protocol_visible_result    脱敏后
display_result             脱敏后或更紧凑版本
log_result                 脱敏后
```

不建议把同一个包含原始 secret 的 `ToolResult` 同时用于模型、wire、UI 和日志。

### 流式输出

`ToolOutputDelta` 不能只对每个 chunk 独立调用正则，因为一个 secret 可能跨 chunk：

```text
chunk 1: sk-123456789
chunk 2: 012345678901234
```

需要一个有界的 streaming redactor buffer，保留 detector 最大匹配长度附近的尾部，只有确认不可能形成跨 chunk secret 后才输出。命令最终结果也要再做一次完整 redaction。

### Redaction report

`RedactionReport` 可以用于 telemetry，但不能把原始 match 文本写入 report、日志或协议。建议只保留：

- detector id。
- count。
- confidence。
- tool name。
- tool call id。

不要记录 secret 的原文、完整 offset 上下文或未脱敏 JSON。

## 七、必须补的测试

### Sandbox

- 原生 `read` 读取 deny path 被拒绝。
- 原生 `write`、`edit`、`apply_patch` 修改 deny path 被拒绝。
- `find`、`grep`、`code_search` 不返回 deny path 内容。
- `skill` 不读取 deny path 下的 `SKILL.md` 或引用文件。
- `DEVO_HOME` 普通状态文件可访问，但 `auth.json` 不可访问。
- pipe、PTY、non-PTY 具有一致的 deny 语义。
- MCP tool 未声明 capability 时不能任意访问 workspace 或网络。

### Redaction

- `ToolResultContent::Text` 脱敏。
- `ToolResultContent::Json` 递归脱敏。
- `ToolResultContent::Mixed` 的 text 和 JSON 都脱敏。
- secret 跨 streaming chunk 时仍能脱敏。
- 多 detector 重叠匹配保持最长/最高 confidence 规则。
- provider request 不包含原始 key。
- ACP raw output、content、TUI transcript 和日志都不包含原始 key。
- redaction report 不包含 secret 原文。
- 没有命中时保持 byte-for-byte 或结构等价，避免无意义改变工具输出。

## 八、建议的实施顺序

1. 先接通 `SecretRedactor` 到 model-visible tool result，并覆盖 Text/JSON/Mixed。
2. 再接通 protocol、ACP、TUI、history 和日志边界。
3. 为原生文件工具抽象统一 filesystem capability checker。
4. 为 `find`、`grep`、`code_search` 接入同一 checker，并修复 `run_rg` 子进程边界。
5. 增加 `auth.json` 特殊 deny 规则和专用 credential 操作路径。
6. 最后处理 skill、MCP 和其他扩展工具的 capability 声明及默认 deny。

## 审计依据

- `crates/sandbox/src/wrap.rs`
- `crates/sandbox/src/profiles.rs`
- `crates/sandbox/src/paths.rs`
- `crates/core/src/tools/shell_exec.rs`
- `crates/core/src/tools/unified_exec/process.rs`
- `crates/utils/process/src/pty/pipe.rs`
- `crates/core/src/tools/handlers/read.rs`
- `crates/core/src/tools/handlers/file_write.rs`
- `crates/core/src/tools/handlers/edit.rs`
- `crates/core/src/tools/handlers/apply_patch.rs`
- `crates/core/src/tools/handlers/ripgrep.rs`
- `crates/core/src/tools/handlers/code_search.rs`
- `crates/core/src/tools/handlers/webfetch.rs`
- `crates/core/src/tools/handlers/websearch.rs`
- `crates/core/src/tools/handlers/skill.rs`
- `crates/safety/src/lib.rs`
- `crates/core/src/logging.rs`
- `crates/config/src/logging.rs`
- `crates/core/src/query/mod.rs`
- `crates/protocol/src/event.rs`
- `crates/protocol/src/acp/event_to_update.rs`
