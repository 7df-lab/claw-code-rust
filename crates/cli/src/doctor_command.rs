//! `devo doctor` health checks for local configuration and provider readiness.
//!
//! This command is intentionally diagnostic rather than mutating: it reports
//! toolchain, config, provider-resolution, and model-catalog state so users can
//! fix setup issues before launching the interactive runtime.

use anyhow::Result;
use devo_core::AppConfigLoader;
use devo_core::FileSystemAppConfigLoader;
use devo_core::PROVIDER_CONFIG_FILE_NAME;
use devo_core::read_provider_catalog_config;
use devo_util_paths::find_devo_home;

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
            let version = String::from_utf8_lossy(&output.stdout);
            println!("  {}", version.trim());
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

    println!("{} Provider config:", "✓".green().bold());
    if let Ok(home) = find_devo_home() {
        let provider_path = home.join(PROVIDER_CONFIG_FILE_NAME);
        if provider_path.exists() {
            println!("  {} {}", "found".green(), provider_path.display());
            match read_provider_catalog_config(&provider_path) {
                Ok(config) => {
                    println!("  providers:    {}", config.providers.len());
                    if let Some(model) = config.model {
                        println!("  default model: {model}");
                    } else {
                        println!("  {} no default model set", "!".yellow());
                    }
                }
                Err(error) => {
                    println!("  {} failed to parse: {error}", "✗".red());
                    all_ok = false;
                }
            }
        } else {
            let config_path = home.join("config.toml");
            if config_path.exists() {
                println!(
                    "  {} {} (legacy; provider writes now use providers.json)",
                    "found".yellow(),
                    config_path.display()
                );
                let content = std::fs::read_to_string(&config_path).unwrap_or_default();
                if has_provider_credentials(&content) {
                    println!("  {} legacy api_key and base_url configured", "✓".green());
                } else {
                    println!("  {} legacy provider settings incomplete", "!".yellow());
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
                    provider_path.display()
                );
                println!("  Run `devo onboard` to create it.");
                all_ok = false;
            }
        }
    }
    println!();

    println!("{} Provider resolution:", "✓".green().bold());
    match find_devo_home() {
        Ok(home) => {
            let cwd = std::env::current_dir()?;
            let app_config = FileSystemAppConfigLoader::new(home.clone()).load(Some(&cwd))?;
            let provider_config = app_config.provider_catalog_config();
            match provider_config.resolve_model(None) {
                Ok(selection) => {
                    let provider = provider_config
                        .providers
                        .get(&selection.provider_id)
                        .expect("resolved provider should be present");
                    let auth = devo_core::read_user_auth_config(
                        &home.join(devo_core::AUTH_CONFIG_FILE_NAME),
                    )?;
                    let api_key = provider
                        .credential
                        .as_deref()
                        .and_then(|credential| auth.credentials.get(credential));
                    println!("  provider:   {}", selection.provider_id);
                    println!(
                        "  model:      {}/{}",
                        selection.provider_id, selection.model_id
                    );
                    println!(
                        "  base_url:   {}",
                        provider.base_url.as_deref().unwrap_or("default")
                    );
                    println!("  wire_api:   {:?}", selection.wire_api);
                    if api_key.is_some() {
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
        }
        Err(e) => {
            println!("  {} {}", "✗".red(), e);
            all_ok = false;
        }
    }
    println!();

    println!("{} Model catalog:", "✓".green().bold());
    let catalog_result: anyhow::Result<_> = find_devo_home()
        .map_err(anyhow::Error::from)
        .and_then(|home| {
            FileSystemAppConfigLoader::new(home)
                .load(None)
                .map_err(anyhow::Error::from)
        })
        .and_then(|config| {
            devo_core::PresetModelCatalog::load_from_provider_config_with_overrides(
                &config.provider_catalog_config(),
                &config.provider.model_overrides,
            )
            .map_err(anyhow::Error::from)
        });
    match catalog_result {
        Ok(catalog) => {
            let count = catalog.into_inner().len();
            println!("  {} provider/model entries loaded", count);
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
