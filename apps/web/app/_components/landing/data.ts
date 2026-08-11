import type { Locale } from "@/lib/locale";

export { localeCookieName, supportedLocales, type Locale } from "@/lib/locale";

export const installCommands = {
  unix:
    "curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh",
  windows: "irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex",
  source: "git clone https://github.com/7df-lab/devo.git\ncd devo\ncargo build --release",
} as const;

export type InstallId = keyof typeof installCommands;

export const landingCopy = {
  en: {
    language: {
      label: "Language",
      en: "EN",
      zh: "中文",
    },
    nav: {
      docs: "Docs",
      install: "Install",
      contact: "Contact",
      github: "GitHub",
    },
    hero: {
      body: "Devo is a lightweight, model-neutral open-source coding agent that runs as a single binary with zero dependencies.",
      primaryCta: "Install Devo",
      secondaryCta: "Read docs",
    },
    comparison: {
      kicker: "Feature matrix",
      title: "A clearer set of tradeoffs.",
      capabilityLabel: "Capability",
      products: ["Devo", "Claude Code", "Droid", "OpenCode"],
      statusLabels: {
        yes: "Yes",
        partial: "Partial",
        no: "No",
      },
      rows: [
        {
          capability: "Open source",
          products: [
            {
              status: "yes",
              evidence: "MIT repo; auditable runtime.",
            },
            {
              status: "no",
              evidence: "Closed source.",
            },
            {
              status: "no",
              evidence: "Closed source.",
            },
            {
              status: "yes",
              evidence: "Open-source project.",
            },
          ],
        },
        {
          capability: "Local semantic code search",
          products: [
            {
              status: "yes",
              evidence: "Optional bundled MCP; off by default.",
            },
            {
              status: "no",
              evidence: "File search only.",
            },
            {
              status: "partial",
              evidence: "Service-backed context.",
            },
            {
              status: "partial",
              evidence: "Grep, glob, LSP.",
            },
          ],
        },
        {
          capability: "Bring your own model provider",
          products: [
            {
              status: "yes",
              evidence: "Cloud and local endpoints.",
            },
            {
              status: "partial",
              evidence: "Anthropic-focused routes.",
            },
            {
              status: "yes",
              evidence: "BYOK and local models.",
            },
            {
              status: "yes",
              evidence: "75+ providers.",
            },
          ],
        },
        {
          capability: "MCP support",
          products: [
            {
              status: "yes",
              evidence: "External tools and context.",
            },
            {
              status: "yes",
              evidence: "First-class MCP.",
            },
            {
              status: "yes",
              evidence: "/mcps and mcp.json.",
            },
            {
              status: "yes",
              evidence: "Local and remote MCP.",
            },
          ],
        },
        {
          capability: "Skill support",
          products: [
            {
              status: "yes",
              evidence: "Skills, scripts, references.",
            },
            {
              status: "yes",
              evidence: "SKILL.md and built-ins.",
            },
            {
              status: "yes",
              evidence: ".factory/skills.",
            },
            {
              status: "yes",
              evidence: "SKILL.md compatible.",
            },
          ],
        },
        {
          capability: "Long-running tasks",
          products: [
            {
              status: "yes",
              evidence: "Multi-turn context.",
            },
            {
              status: "yes",
              evidence: "Sessions, compaction, /loop.",
            },
            {
              status: "yes",
              evidence: "Mission milestones.",
            },
            {
              status: "partial",
              evidence: "Sessions and compaction.",
            },
          ],
        },
        {
          capability: "Multi-agent support",
          products: [
            {
              status: "yes",
              evidence: "Specialized agents.",
            },
            {
              status: "yes",
              evidence: "Subagents and /batch.",
            },
            {
              status: "yes",
              evidence: "Custom droids + Missions.",
            },
            {
              status: "yes",
              evidence: "Agents and child sessions.",
            },
          ],
        },
        {
          capability: "Plan mode",
          products: [
            {
              status: "yes",
              evidence: "Steps before edits.",
            },
            {
              status: "yes",
              evidence: "Proposal before writes.",
            },
            {
              status: "yes",
              evidence: "Specification Mode.",
            },
            {
              status: "yes",
              evidence: "Restricted Plan agent.",
            },
          ],
        },
        {
          capability: "Parallel tool calls",
          products: [
            {
              status: "yes",
              evidence: "Parallel tool execution.",
            },
            {
              status: "partial",
              evidence: "Session/subagent parallelism.",
            },
            {
              status: "partial",
              evidence: "Parallel delegated work.",
            },
            {
              status: "partial",
              evidence: "Parallel subagents.",
            },
          ],
        },
        {
          capability: "Lightweight memory footprint",
          products: [
            {
              status: "yes",
              evidence: "Compact Rust runtime.",
            },
            {
              status: "partial",
              evidence: "Native binary + extension context.",
            },
            {
              status: "partial",
              evidence: "Service-backed memory.",
            },
            {
              status: "partial",
              evidence: "Compaction and pruning.",
            },
          ],
        },
        {
          capability: "Zero dependencies",
          products: [
            {
              status: "yes",
              evidence: "Single binary path.",
            },
            {
              status: "partial",
              evidence: "Node during install.",
            },
            {
              status: "no",
              evidence: "No zero-dependency claim.",
            },
            {
              status: "no",
              evidence: "Package-manager install paths.",
            },
          ],
        },
      ],
    },
    proofRows: [
      {
        eyebrow: "Zero-dependency runtime",
        title: "One binary, less operational drag",
        body: "Devo keeps the agent runtime lightweight, so teams can start without adding a service, daemon, or dependency chain.",
      },
      {
        eyebrow: "Model-neutral by design",
        title: "Use the model provider that fits the job",
        body: "Bring cloud providers, local endpoints, and custom model routes into one workflow instead of locking development to a single vendor.",
      },
      {
        eyebrow: "Long-running tasks",
        title: "/goal keeps large tasks moving",
        body: "Set a goal once, and Devo can carry context forward until the larger change is complete.",
      },
    ],
    workflow: {
      kicker: "Beyond coding",
      title: "One agent runtime for downstream engineering work.",
      body: "Devo can extend into deep research, security audits, test generation, verification, and repository governance, connecting context gathering, execution, and review in one traceable flow.",
      imageAlt: "Devo command-line visual style",
    },
    enterprise: {
      kicker: "Enterprise",
      title: "Built for enterprise teams from day one.",
      body: "For organizations running Devo across many repositories, Devo can provide monitoring, analytics, repository quality checks, and security analysis so platform teams can see adoption, efficiency, and risk in one place.",
      features: [
        {
          title: "Monitoring and analytics",
          body: "Track agent usage, model mix, tool calls, and team adoption across projects.",
        },
        {
          title: "Repository quality signals",
          body: "Measure maintainability, test coverage, dependency health, and readiness for autonomous work.",
        },
        {
          title: "Security and governance",
          body: "Surface risky changes, policy gaps, and repository-level security issues before they spread.",
        },
      ],
      footer:
        "Enterprise deployments can include private infrastructure, SSO/SAML, audit logs, policy controls, and custom reporting.",
      dashboard: {
        label: "Enterprise insights",
        title: "Devo operational overview",
        period: "Last 30 days",
        exportLabel: "Export",
        usageLabels: [
          "Agent usage by model",
          "Requests, tool calls, and model routing over time.",
          "Updated 8 min ago",
          "gpt-5-codex",
          "claude-sonnet",
          "local-qwen",
          "gpt-5",
          "other",
        ],
        metrics: [
          { label: "Goal completions", value: "1.8K", delta: "+18%" },
          { label: "Tool calls", value: "667K", delta: "+12%" },
          { label: "File ops", value: "124K", delta: "+9%" },
          { label: "Active repos", value: "114", delta: "+6" },
        ],
        qualityTitle: "Repository quality",
        qualityScore: "82",
        qualityRows: [
          { label: "Tests", value: 84 },
          { label: "Docs", value: 76 },
          { label: "Deps", value: 88 },
          { label: "Review", value: 79 },
        ],
        securityTitle: "Security analysis",
        securityRows: [
          {
            label: "High-risk dependency drift",
            body: "6 repositories need owner review.",
            severity: "High",
          },
          {
            label: "Policy coverage gaps",
            body: "Missing checks on release branches.",
            severity: "Med",
          },
          {
            label: "Secrets scanning clean",
            body: "No new exposed credentials detected.",
            severity: "Clear",
          },
        ],
        teamTitle: "Team adoption",
        teamRows: [
          { team: "Platform Engineering", repos: "32 repos", score: "90", trend: "+21%" },
          { team: "Product Infrastructure", repos: "28 repos", score: "85", trend: "+17%" },
          { team: "Data Systems", repos: "18 repos", score: "79", trend: "+14%" },
          { team: "Security", repos: "9 repos", score: "88", trend: "+11%" },
        ],
      },
    },
    install: {
      terminalTitle: "devo install",
      tabAria: "Install options",
      copy: "Copy",
      copied: "Copied",
      tabs: {
        unix: "macOS / Linux",
        windows: "Windows",
        source: "Source",
      },
    },
    closing: {
      kicker: "Follow the project",
      title: "Open, early, and moving quickly.",
      bodyBeforeEmail:
        "Devo is moving quickly. For partnerships, enterprise deployments, hiring, or investment conversations, contact",
      locationLabel: "Location",
      location: "Beijing, China",
      openDocs: "Open docs",
      viewGithub: "View GitHub",
    },
  },
  zh: {
    language: {
      label: "语言",
      en: "EN",
      zh: "中文",
    },
    nav: {
      docs: "文档",
      install: "安装",
      contact: "联系",
      github: "GitHub",
    },
    hero: {
      body: "Devo 是一个轻量、模型中立的开源编程智能体，单二进制运行无任何依赖。",
      primaryCta: "安装 Devo",
      secondaryCta: "阅读文档",
    },
    comparison: {
      kicker: "横向对比",
      title: "Devo 的取舍更清楚。",
      capabilityLabel: "能力",
      products: ["Devo", "Claude Code", "Droid", "OpenCode"],
      statusLabels: {
        yes: "支持",
        partial: "部分",
        no: "不支持",
      },
      rows: [
        {
          capability: "是否开源",
          products: [
            {
              status: "yes",
              evidence: "MIT 仓库，可审计。",
            },
            {
              status: "no",
              evidence: "闭源。",
            },
            {
              status: "no",
              evidence: "闭源。",
            },
            {
              status: "yes",
              evidence: "开源项目。",
            },
          ],
        },
        {
          capability: "本地代码语义搜索",
          products: [
            {
              status: "yes",
              evidence: "内置 MCP，默认关闭。",
            },
            {
              status: "no",
              evidence: "文件搜索为主。",
            },
            {
              status: "partial",
              evidence: "服务化上下文。",
            },
            {
              status: "partial",
              evidence: "grep / glob / LSP。",
            },
          ],
        },
        {
          capability: "接入您的任意模型",
          products: [
            {
              status: "yes",
              evidence: "云端与本地 endpoint。",
            },
            {
              status: "partial",
              evidence: "Anthropic 生态为主。",
            },
            {
              status: "yes",
              evidence: "BYOK 与本地模型。",
            },
            {
              status: "yes",
              evidence: "75+ 提供方。",
            },
          ],
        },
        {
          capability: "MCP 支持",
          products: [
            {
              status: "yes",
              evidence: "外部工具和上下文。",
            },
            {
              status: "yes",
              evidence: "一等 MCP 扩展。",
            },
            {
              status: "yes",
              evidence: "/mcps 与 mcp.json。",
            },
            {
              status: "yes",
              evidence: "本地和远程 MCP。",
            },
          ],
        },
        {
          capability: "Skill 支持",
          products: [
            {
              status: "yes",
              evidence: "Skills、脚本、资料。",
            },
            {
              status: "yes",
              evidence: "SKILL.md 与内置 skills。",
            },
            {
              status: "yes",
              evidence: ".factory/skills。",
            },
            {
              status: "yes",
              evidence: "兼容 SKILL.md。",
            },
          ],
        },
        {
          capability: "长程任务",
          products: [
            {
              status: "yes",
              evidence: "/goal。",
            },
            {
              status: "yes",
              evidence: "session、压缩、/loop。",
            },
            {
              status: "yes",
              evidence: "Mission 里程碑。",
            },
            {
              status: "partial",
              evidence: "session 与压缩。",
            },
          ],
        },
        {
          capability: "Multi-agent 支持",
          products: [
            {
              status: "yes",
              evidence: "专门 agent 分工。",
            },
            {
              status: "yes",
              evidence: "subagents 与 /batch。",
            },
            {
              status: "yes",
              evidence: "Droids + Missions。",
            },
            {
              status: "yes",
              evidence: "agents 与子 session。",
            },
          ],
        },
        {
          capability: "Plan 模式",
          products: [
            {
              status: "yes",
              evidence: "先规划再修改。",
            },
            {
              status: "yes",
              evidence: "先提案再落盘。",
            },
            {
              status: "yes",
              evidence: "Specification Mode。",
            },
            {
              status: "yes",
              evidence: "受限 Plan agent。",
            },
          ],
        },
        {
          capability: "并行工具调用",
          products: [
            {
              status: "yes",
              evidence: "工具可并行执行。",
            },
            {
              status: "partial",
              evidence: "session / subagent 并行。",
            },
            {
              status: "partial",
              evidence: "委派任务并行。",
            },
            {
              status: "partial",
              evidence: "subagents 并行。",
            },
          ],
        },
        {
          capability: "轻量内存占用",
          products: [
            {
              status: "yes",
              evidence: "紧凑 Rust runtime。",
            },
            {
              status: "partial",
              evidence: "原生二进制 + 扩展上下文。",
            },
            {
              status: "partial",
              evidence: "服务化记忆。",
            },
            {
              status: "partial",
              evidence: "压缩与 pruning。",
            },
          ],
        },
        {
          capability: "零依赖",
          products: [
            {
              status: "yes",
              evidence: "单二进制路径。",
            },
            {
              status: "partial",
              evidence: "安装需 Node。",
            },
            {
              status: "no",
              evidence: "未声明零依赖。",
            },
            {
              status: "no",
              evidence: "依赖包管理器路径。",
            },
          ],
        },
      ],
    },
    proofRows: [
      {
        eyebrow: "零依赖运行",
        title: "安装方便，极其轻量",
        body: "单二进制即可运行，几乎无需额外依赖，启动和使用都足够快速。",
      },
      {
        eyebrow: "模型中立",
        title: "接入您的任何模型",
        body: "云端 provider、本地 endpoint 和自定义模型路由可以共用一套配置方式，不把项目锁在单一供应商里。",
      },
      {
        eyebrow: "长程任务",
        title: "/goal 一口气推进大型任务",
        body: "设定目标后，Devo 可以持续推进上下文，直到大型改动完整完成。",
      },
    ],
    workflow: {
      kicker: "不止于编程",
      title: "一套 agent runtime，延展到更多工程场景。",
      body: "Devo 可以用于深度调研、安全审计、测试生成与验证、仓库治理等下游场景，将上下文收集、任务执行和结果复核连接成可追踪的链路。",
      imageAlt: "Devo 命令行视觉风格",
    },
    enterprise: {
      kicker: "企业级能力",
      title: "从第一天开始为企业用户构建",
      body: "如果您是企业用户，Devo 可以提供使用监控、效率分析、软件仓库质量评估和安全风险分析，帮助平台团队统一看清采用情况、交付效率和治理风险。",
      features: [
        {
          title: "全面监控与分析",
          body: "按团队、工程师、模型和项目追踪 agent 使用情况、工具调用和采用趋势。",
        },
        {
          title: "仓库质量洞察",
          body: "持续评估可维护性、测试覆盖、依赖健康度和自主开发就绪度。",
        },
        {
          title: "安全与治理保障",
          body: "识别高风险变更、策略缺口和仓库级安全问题，便于企业集中治理。",
        },
      ],
      footer:
        "企业部署可扩展私有化基础设施、SSO/SAML、审计日志、策略控制和定制化报表。",
      dashboard: {
        label: "企业洞察",
        title: "Devo 运营总览",
        period: "过去 30 天",
        exportLabel: "导出",
        usageLabels: [
          "按模型统计 agent 使用",
          "追踪请求、工具调用和模型路由趋势。",
          "8 分钟前更新",
          "gpt-5-codex",
          "claude-sonnet",
          "local-qwen",
          "gpt-5",
          "其他",
        ],
        metrics: [
          { label: "Goal 完成数", value: "1.8K", delta: "+18%" },
          { label: "工具调用", value: "667K", delta: "+12%" },
          { label: "文件操作", value: "124K", delta: "+9%" },
          { label: "活跃仓库", value: "114", delta: "+6" },
        ],
        qualityTitle: "仓库质量",
        qualityScore: "82",
        qualityRows: [
          { label: "测试", value: 84 },
          { label: "文档", value: 76 },
          { label: "依赖", value: 88 },
          { label: "评审", value: 79 },
        ],
        securityTitle: "安全分析",
        securityRows: [
          {
            label: "高风险依赖漂移",
            body: "6 个仓库需要负责人复核。",
            severity: "高",
          },
          {
            label: "策略覆盖缺口",
            body: "发布分支缺少必要检查。",
            severity: "中",
          },
          {
            label: "密钥扫描正常",
            body: "未发现新增暴露凭据。",
            severity: "清洁",
          },
        ],
        teamTitle: "团队采用",
        teamRows: [
          { team: "平台工程团队", repos: "32 仓库", score: "90", trend: "+21%" },
          { team: "产品基础设施", repos: "28 仓库", score: "85", trend: "+17%" },
          { team: "数据系统团队", repos: "18 仓库", score: "79", trend: "+14%" },
          { team: "安全团队", repos: "9 仓库", score: "88", trend: "+11%" },
        ],
      },
    },
    install: {
      terminalTitle: "devo install",
      tabAria: "安装选项",
      copy: "复制",
      copied: "已复制",
      tabs: {
        unix: "macOS / Linux",
        windows: "Windows",
        source: "源码构建",
      },
    },
    closing: {
      kicker: "关注项目",
      title: "开放、早期，并且快速迭代。",
      bodyBeforeEmail:
        "Devo 正在持续迭代。如果你想聊合作、企业部署、招聘或投资，欢迎联系",
      locationLabel: "所在地",
      location: "Beijing, China",
      openDocs: "打开文档",
      viewGithub: "查看 GitHub",
    },
  },
} as const;

export type LandingCopy = (typeof landingCopy)[Locale];
