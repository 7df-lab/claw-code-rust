use anyhow::Result;
use devo_core::resolve_provider_settings;
use devo_utils::find_devo_home;

pub(crate) async fn run_doctor() -> Result<()> {
    use colored::Colorize;
    use std::process::Command;

    println!("{}", "=== Devo Doctor ===".bold());
    println!();

    let mut all_ok = true;

    println!("{} Rust toolchain:", "✓".green().bold());
    let rustc = Command::new("rustc").arg("--version").output();
    match rustc {
        Ok(output) => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            println!("  {}", version);
        }
        Err(e) => {
            println!("  {} rustc not found: {}", "✗".red(), e);
            all_ok = false;
        }
    }
    println!();

    println!("{} Config home (DEVO_HOME):", "✓".green().bold());
    match find_devo_home() {
        Ok(home) => {
            println!("  {}", home.display());
        }
        Err(e) => {
            println!("  {} {}", "✗".red(), e);
            all_ok = false;
        }
    }
    println!();

    println!("{} Config file:", "✓".green().bold());
    if let Ok(home) = find_devo_home() {
        let config_path = home.join("config.toml");
        if config_path.exists() {
            println!("  {} {}", "found".green(), config_path.display());
            let content = std::fs::read_to_string(&config_path).unwrap_or_default();
            if has_provider_credentials(&content) {
                println!("  {} api_key and base_url configured", "✓".green());
            } else {
                println!("  {} api_key or base_url missing", "!".yellow());
                all_ok = false;
            }
            if let Some(line) = default_model_line(&content) {
                println!("  default model: {}", line.trim());
            } else {
                println!("  {} no default model set", "!".yellow());
            }
        } else {
            println!(
                "  {} not found at {}",
                "missing".yellow(),
                config_path.display()
            );
            println!("  Run `devo onboard` to create it.");
            all_ok = false;
        }
    }
    println!();

    println!("{} Provider resolution:", "✓".green().bold());
    match resolve_provider_settings() {
        Ok(resolved) => {
            println!("  provider:   {}", resolved.provider_id);
            println!("  model:      {}", resolved.model);
            println!(
                "  base_url:   {}",
                resolved.base_url.unwrap_or("default".into())
            );
            println!("  wire_api:   {:?}", resolved.wire_api);
            if resolved.api_key.is_some() {
                println!("  api_key:    {} (set)", "✓".green());
            } else {
                println!("  api_key:    {} (not set)", "✗".red());
                all_ok = false;
            }
        }
        Err(e) => {
            println!("  {} {}", "✗".red(), e);
            all_ok = false;
        }
    }
    println!();

    println!("{} Model catalog:", "✓".green().bold());
    match devo_core::PresetModelCatalog::load() {
        Ok(catalog) => {
            let count = catalog.into_inner().len();
            println!("  {} builtin models loaded", count);
        }
        Err(e) => {
            println!("  {} failed to load: {}", "✗".red(), e);
            all_ok = false;
        }
    }
    println!();

    if all_ok {
        println!("{}", "All checks passed. Ready to use!".green().bold());
    } else {
        println!(
            "{}",
            "Some checks failed. See above for details.".yellow().bold()
        );
        std::process::exit(1);
    }

    Ok(())
}

fn has_provider_credentials(config_content: &str) -> bool {
    config_content.contains("api_key") && config_content.contains("base_url")
}

fn default_model_line(config_content: &str) -> Option<&str> {
    config_content.lines().find(|line| {
        let Some(rest) = line.trim_start().strip_prefix("model") else {
            return false;
        };
        rest.trim_start().starts_with('=')
    })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::default_model_line;
    use super::has_provider_credentials;

    #[test]
    fn has_provider_credentials_requires_api_key_and_base_url() {
        for (content, expected) in [
            ("api_key = 'key'\nbase_url = 'https://api.example'\n", true),
            ("api_key = 'key'\n", false),
            ("base_url = 'https://api.example'\n", false),
            ("", false),
        ] {
            assert_eq!(has_provider_credentials(content), expected);
        }
    }

    #[test]
    fn default_model_line_finds_exact_model_assignment() {
        for (content, expected) in [
            ("model = 'gpt-test'\n", Some("model = 'gpt-test'")),
            ("  model = 'gpt-test'\n", Some("  model = 'gpt-test'")),
            ("model='gpt-test'\n", Some("model='gpt-test'")),
            ("model_provider = 'openai'\n", None),
            ("default_model = 'gpt-test'\n", None),
            ("", None),
        ] {
            assert_eq!(default_model_line(content), expected);
        }
    }
}
