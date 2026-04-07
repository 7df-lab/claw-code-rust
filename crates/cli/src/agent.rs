use std::io::{self, Write};

use anyhow::Result;
use clap::Args;
use clawcr_core::BuiltinModelCatalog;
use clawcr_safety::legacy_permissions::PermissionMode;
use clawcr_server::{
    InputItem, ItemEnvelope, ItemKind, ServerEvent, SessionStartParams, StdioServerClient,
    StdioServerClientConfig, TurnStartParams,
};
use clawcr_tui::{run_interactive_tui, InteractiveTuiConfig};

use crate::config;

/// Output format for non-interactive (print/query) mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Plain text — assistant text only, streamed to stdout.
    Text,
    /// Newline-delimited JSON events (one JSON object per line).
    StreamJson,
    /// Single JSON object written after the turn completes.
    Json,
}

impl std::str::FromStr for OutputFormat {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "text" => Ok(OutputFormat::Text),
            "stream-json" => Ok(OutputFormat::StreamJson),
            "json" => Ok(OutputFormat::Json),
            other => anyhow::bail!("unknown output format '{}' (text|stream-json|json)", other),
        }
    }
}

/// Common agent-facing flags accepted by the main `clawcr` command.
#[derive(Debug, Args)]
pub struct AgentCli {
    /// Model to use for future turns.
    #[arg(short, long)]
    pub model: Option<String>,

    /// System prompt placeholder retained for CLI compatibility.
    #[arg(
        short,
        long,
        default_value = "You are a helpful coding assistant. \
        Use tools when appropriate to help the user. Be concise."
    )]
    pub system: String,

    /// Permission mode: auto, interactive, deny.
    #[arg(short, long, default_value = "auto")]
    pub permission: String,

    /// Run a single prompt non-interactively then exit.
    #[arg(short = 'q', long)]
    pub query: Option<String>,

    /// Run a single prompt non-interactively then exit (alias for --query).
    #[arg(long)]
    pub print: Option<String>,

    /// Output format for non-interactive mode: text (default), stream-json, json.
    #[arg(long, default_value = "text")]
    pub output_format: OutputFormat,

    /// Maximum turns placeholder retained for CLI compatibility.
    #[arg(long, default_value = "100")]
    pub max_turns: usize,

    /// Provider: anthropic, ollama, openai (auto-detected if not set).
    #[arg(long)]
    pub provider: Option<String>,

    /// Ollama server URL.
    #[arg(long, default_value = "http://localhost:11434")]
    pub ollama_url: String,
}

/// Runs the interactive or one-shot coding-agent entrypoint.
pub async fn run_agent(cli: AgentCli, force_onboarding: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let single_prompt = cli.query.or(cli.print);
    let interactive = single_prompt.is_none();

    let permission_mode = match cli.permission.as_str() {
        "auto" => PermissionMode::AutoApprove,
        "interactive" => PermissionMode::Interactive,
        "deny" => PermissionMode::Deny,
        other => {
            eprintln!("unknown permission mode '{other}', using auto");
            PermissionMode::AutoApprove
        }
    };

    if cli.provider.as_deref() == Some("ollama") {
        config::ensure_ollama(&cli.ollama_url, interactive)?;
    }

    let resolved = config::resolve_provider_settings(
        cli.provider.as_deref(),
        cli.model.as_deref(),
        &cli.ollama_url,
        interactive,
    )?;
    let server_env = server_env_overrides(&resolved);
    let model_catalog = BuiltinModelCatalog::load()?;
    let stored_config = config::load_config().unwrap_or_default();
    let show_model_onboarding =
        interactive && (force_onboarding || (cli.model.is_none() && stored_config.model.is_none()));

    if interactive {
        run_interactive_tui(InteractiveTuiConfig {
            model: resolved.model,
            cwd,
            server_env,
            startup_prompt: None,
            model_catalog,
            show_model_onboarding,
        })
        .await?;
        return Ok(());
    }

    if let Some(prompt) = single_prompt {
        run_one_shot_via_server(
            &cwd,
            &resolved.model,
            server_env,
            cli.output_format,
            prompt,
            permission_mode,
        )
        .await?;
    }
    Ok(())
}

fn server_env_overrides(resolved: &config::ResolvedProviderSettings) -> Vec<(String, String)> {
    let mut env = vec![
        ("CLAWCR_PROVIDER".to_string(), resolved.provider.clone()),
        ("CLAWCR_MODEL".to_string(), resolved.model.clone()),
    ];
    if let Some(base_url) = &resolved.base_url {
        env.push(("CLAWCR_BASE_URL".to_string(), base_url.clone()));
    }
    if let Some(api_key) = &resolved.api_key {
        env.push(("CLAWCR_API_KEY".to_string(), api_key.clone()));
    }
    env
}

async fn run_one_shot_via_server(
    cwd: &std::path::Path,
    model: &str,
    server_env: Vec<(String, String)>,
    output_format: OutputFormat,
    prompt: String,
    permission_mode: PermissionMode,
) -> Result<()> {
    let approval_policy = match permission_mode {
        PermissionMode::AutoApprove => Some("auto".to_string()),
        PermissionMode::Interactive => Some("interactive".to_string()),
        PermissionMode::Deny => Some("deny".to_string()),
    };
    let mut client = StdioServerClient::spawn(StdioServerClientConfig {
        program: std::env::current_exe()?,
        workspace_root: Some(cwd.to_path_buf()),
        env: server_env,
    })
    .await?;
    let _ = client.initialize().await?;
    let session = client
        .session_start(SessionStartParams {
            cwd: cwd.to_path_buf(),
            ephemeral: true,
            title: None,
            model: Some(model.to_string()),
        })
        .await?;
    let _ = client
        .turn_start(TurnStartParams {
            session_id: session.session_id,
            input: vec![InputItem::Text { text: prompt }],
            model: Some(model.to_string()),
            sandbox: None,
            approval_policy,
            cwd: None,
        })
        .await?;

    let mut assistant_text = String::new();
    while let Some((method, event)) = client.recv_event().await? {
        match event {
            ServerEvent::ItemDelta { payload, .. } if method == "item/agentMessage/delta" => {
                assistant_text.push_str(&payload.delta);
                handle_text_delta(output_format, &payload.delta);
            }
            ServerEvent::ItemCompleted(payload) => {
                handle_completed_item(output_format, payload.item);
            }
            ServerEvent::TurnCompleted(payload) => {
                if output_format == OutputFormat::Text {
                    println!();
                } else if output_format == OutputFormat::StreamJson {
                    println!(
                        "{}",
                        serde_json::json!({
                            "type": "turn_complete",
                            "stop_reason": format!("{:?}", payload.turn.status),
                        })
                    );
                } else {
                    println!(
                        "{}",
                        serde_json::json!({
                            "type": "result",
                            "text": assistant_text,
                            "session_id": session.session_id,
                            "input_tokens": 0,
                            "output_tokens": 0,
                        })
                    );
                }
                break;
            }
            ServerEvent::TurnFailed(payload) => {
                anyhow::bail!("turn failed with status {:?}", payload.turn.status);
            }
            _ => {}
        }
    }
    client.shutdown().await?;
    Ok(())
}

fn handle_text_delta(output_format: OutputFormat, text: &str) {
    match output_format {
        OutputFormat::Text => {
            print!("{text}");
            let _ = io::stdout().flush();
        }
        OutputFormat::StreamJson => {
            println!(
                "{}",
                serde_json::json!({ "type": "text_delta", "text": text })
            );
        }
        OutputFormat::Json => {}
    }
}

fn handle_completed_item(output_format: OutputFormat, item: ItemEnvelope) {
    match item {
        ItemEnvelope {
            item_kind: ItemKind::ToolCall,
            payload,
            ..
        } => {
            let name = payload
                .get("tool_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tool");
            match output_format {
                OutputFormat::Text => eprintln!("\n⚡ calling tool: {name}"),
                OutputFormat::StreamJson => println!(
                    "{}",
                    serde_json::json!({
                        "type": "tool_use_start",
                        "id": payload.get("tool_use_id").cloned().unwrap_or(serde_json::Value::Null),
                        "name": name,
                    })
                ),
                OutputFormat::Json => eprintln!("⚡ calling tool: {name}"),
            }
        }
        ItemEnvelope {
            item_kind: ItemKind::ToolResult,
            payload,
            ..
        } => {
            let content = payload
                .get("content")
                .map(render_json_value_text)
                .unwrap_or_default();
            let is_error = payload
                .get("is_error")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            match output_format {
                OutputFormat::Text => {
                    if is_error {
                        eprintln!("❌ tool error: {}", truncate(&content, 200));
                    } else {
                        eprintln!("✅ tool done ({})", byte_summary(&content));
                    }
                }
                OutputFormat::StreamJson => println!(
                    "{}",
                    serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": payload.get("tool_use_id").cloned().unwrap_or(serde_json::Value::Null),
                        "content": content,
                        "is_error": is_error,
                    })
                ),
                OutputFormat::Json => {
                    if is_error {
                        eprintln!("❌ tool error: {}", truncate(&content, 200));
                    }
                }
            }
        }
        _ => {}
    }
}

fn render_json_value_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        _ => value.to_string(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

fn byte_summary(s: &str) -> String {
    let len = s.len();
    if len < 1024 {
        format!("{len} bytes")
    } else {
        format!("{:.1} KB", len as f64 / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn truncate_ascii_within_limit() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_ascii_at_limit() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn truncate_ascii_over_limit() {
        assert_eq!(truncate("hello world", 5), "hello...");
    }

    #[test]
    fn truncate_multibyte_at_char_boundary() {
        assert_eq!(truncate("café", 4), "caf...");
    }

    #[test]
    fn truncate_multibyte_inside_char() {
        assert_eq!(truncate("a中b", 2), "a...");
    }

    #[test]
    fn truncate_cjk_string() {
        assert_eq!(truncate("你好世界", 7), "你好...");
    }

    #[test]
    fn truncate_emoji() {
        assert_eq!(truncate("hi😀bye", 4), "hi...");
    }

    #[test]
    fn truncate_japanese() {
        assert_eq!(truncate("こんにちは", 8), "こん...");
    }

    #[test]
    fn truncate_mixed_cjk_error_output() {
        let input = "error[E0308]: エラー: 型が一致しません expected `i32`, found `&str`";
        let result = truncate(input, 30);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 33 + 3);
    }

    #[test]
    fn truncate_empty() {
        assert_eq!(truncate("", 10), "");
    }
}
