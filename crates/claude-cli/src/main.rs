use std::io::{self, BufRead, Write};
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;

use agent_core::{query, Message, QueryEvent, SessionConfig, SessionState};
use agent_permissions::PermissionMode;
use agent_provider::ModelProvider;
use agent_tools::{ToolOrchestrator, ToolRegistry};

/// Claude Code Rust — a modular agent runtime.
#[derive(Parser, Debug)]
#[command(name = "claude", version, about)]
struct Cli {
    /// Model to use
    #[arg(short, long, default_value = "claude-sonnet-4-20250514")]
    model: String,

    /// System prompt
    #[arg(short, long, default_value = "You are a helpful coding assistant. \
        Use tools when appropriate to help the user. Be concise.")]
    system: String,

    /// Permission mode: auto, interactive, deny
    #[arg(short, long, default_value = "auto")]
    permission: String,

    /// Run a single prompt non-interactively then exit
    #[arg(short = 'q', long)]
    query: Option<String>,

    /// Maximum turns per conversation
    #[arg(long, default_value = "100")]
    max_turns: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;

    let config = SessionConfig {
        model: cli.model.clone(),
        system_prompt: cli.system.clone(),
        max_turns: cli.max_turns,
        ..Default::default()
    };

    let _perm_mode = match cli.permission.as_str() {
        "auto" => PermissionMode::AutoApprove,
        "interactive" => PermissionMode::Interactive,
        "deny" => PermissionMode::Deny,
        other => {
            eprintln!("unknown permission mode '{}', using auto", other);
            PermissionMode::AutoApprove
        }
    };

    // Register tools
    let mut registry = ToolRegistry::new();
    tools_builtin::register_builtin_tools(&mut registry);
    let registry = Arc::new(registry);
    let orchestrator = ToolOrchestrator::new(Arc::clone(&registry));

    // Pick provider based on environment
    let provider: Box<dyn ModelProvider> = match std::env::var("ANTHROPIC_API_KEY") {
        Ok(key) if !key.is_empty() => {
            eprintln!("Using Anthropic API (model: {})", cli.model);
            Box::new(agent_provider::anthropic::AnthropicProvider::new(key))
        }
        _ => {
            eprintln!("⚠ ANTHROPIC_API_KEY not set — running with stub provider.");
            eprintln!("  Set the env var to use a real model.\n");
            Box::new(StubProvider)
        }
    };

    let mut session = SessionState::new(config, cwd);

    let on_event: Arc<dyn Fn(QueryEvent) + Send + Sync> = Arc::new(|event| match event {
        QueryEvent::TextDelta(text) => {
            print!("{}", text);
            let _ = io::stdout().flush();
        }
        QueryEvent::ToolUseStart { name, .. } => {
            eprintln!("\n⚡ calling tool: {}", name);
        }
        QueryEvent::ToolResult {
            is_error, content, ..
        } => {
            if is_error {
                eprintln!("❌ tool error: {}", truncate(&content, 200));
            } else {
                eprintln!("✅ tool done ({})", byte_summary(&content));
            }
        }
        QueryEvent::TurnComplete { .. } => {
            println!();
        }
        QueryEvent::Usage {
            input_tokens,
            output_tokens,
        } => {
            eprintln!(
                "  [tokens: {} in / {} out]",
                input_tokens, output_tokens
            );
        }
    });

    // Single-query mode
    if let Some(prompt) = cli.query {
        session.push_message(Message::user(prompt));
        query(
            &mut session,
            provider.as_ref(),
            Arc::clone(&registry),
            &orchestrator,
            Some(on_event),
        )
        .await?;
        return Ok(());
    }

    // Interactive REPL
    println!("Claude Code Rust v{}", env!("CARGO_PKG_VERSION"));
    println!("Type your message, or 'exit' / Ctrl-D to quit.\n");

    let stdin = io::stdin();
    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "exit" || line == "quit" {
            break;
        }

        session.push_message(Message::user(line));

        if let Err(e) = query(
            &mut session,
            provider.as_ref(),
            Arc::clone(&registry),
            &orchestrator,
            Some(Arc::clone(&on_event)),
        )
        .await
        {
            eprintln!("error: {}", e);
        }
    }

    eprintln!(
        "\n[session: {} turns, {} in / {} out tokens]",
        session.turn_count, session.total_input_tokens, session.total_output_tokens
    );

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

fn byte_summary(s: &str) -> String {
    let len = s.len();
    if len < 1024 {
        format!("{} bytes", len)
    } else {
        format!("{:.1} KB", len as f64 / 1024.0)
    }
}

// ---------------------------------------------------------------------------
// Stub provider — fallback when no API key is configured
// ---------------------------------------------------------------------------

use std::pin::Pin;
use futures::Stream;
use agent_provider::{
    ModelRequest, ModelResponse, ResponseContent, StopReason, StreamEvent, Usage,
};

struct StubProvider;

#[async_trait::async_trait]
impl ModelProvider for StubProvider {
    async fn complete(&self, _request: ModelRequest) -> anyhow::Result<ModelResponse> {
        Ok(ModelResponse {
            id: "stub".into(),
            content: vec![ResponseContent::Text(
                "[stub provider] Set ANTHROPIC_API_KEY to enable real model calls.".into(),
            )],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
        })
    }

    async fn stream(
        &self,
        request: ModelRequest,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>> {
        let response = self.complete(request).await?;
        let events = vec![
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: match &response.content[0] {
                    ResponseContent::Text(t) => t.clone(),
                    _ => String::new(),
                },
            }),
            Ok(StreamEvent::MessageDone { response }),
        ];
        Ok(Box::pin(futures::stream::iter(events)))
    }

    fn name(&self) -> &str {
        "stub"
    }
}
