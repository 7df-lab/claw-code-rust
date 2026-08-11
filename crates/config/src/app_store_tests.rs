use pretty_assertions::assert_eq;
use std::fs;
use std::time::SystemTime;

use super::APP_CONFIG_FILE_NAME;
use super::AppConfigStore;

fn unique_temp_dir(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "devo-config-{label}-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn model_config_option_writes_model_default_projection() {
    let root = unique_temp_dir("model-default");
    let home = root.join(".devo");
    fs::create_dir_all(&home).expect("create config dir");
    fs::write(
        home.join(APP_CONFIG_FILE_NAME),
        r#"
model_provider = "openai"
model = "test-model"

[defaults]
model_binding = "test-binding"

[providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
credential = "openai_api_key"
wire_apis = ["openai_chat_completions"]
enabled = true

[model_bindings.test-binding]
model_slug = "test-model"
provider = "openai"
request_model = "test-model"
invocation_method = "openai_chat_completions"
enabled = true

[model_bindings.alt-binding]
model_slug = "alt-model"
provider = "openai"
request_model = "alt-model"
invocation_method = "openai_chat_completions"
enabled = true
"#,
    )
    .expect("write config");

    let mut store = AppConfigStore::load(home.clone(), None).expect("load config");
    store
        .set_model_config_option("model", "alt-binding")
        .expect("write model default");

    let config_text = fs::read_to_string(home.join(APP_CONFIG_FILE_NAME)).expect("read config");
    let document: toml::Value = toml::from_str(&config_text).expect("parse config");
    assert_eq!(
        document["defaults"]["model_binding"].as_str(),
        Some("alt-binding")
    );
    assert_eq!(document["model_provider"].as_str(), Some("openai"));
    assert_eq!(document["model"].as_str(), Some("test-model"));
    assert_eq!(
        store
            .effective_config()
            .provider
            .defaults
            .model_binding
            .as_deref(),
        Some("alt-binding")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn model_config_option_writes_reasoning_effort_selection() {
    let root = unique_temp_dir("reasoning-default");
    let home = root.join(".devo");
    fs::create_dir_all(&home).expect("create config dir");
    fs::write(
        home.join(APP_CONFIG_FILE_NAME),
        r#"
model_reasoning_effort_selection = "medium"
"#,
    )
    .expect("write config");

    let mut store = AppConfigStore::load(home.clone(), None).expect("load config");
    store
        .set_model_config_option("thought_level", "high")
        .expect("write reasoning default");

    let config_text = fs::read_to_string(home.join(APP_CONFIG_FILE_NAME)).expect("read config");
    let document: toml::Value = toml::from_str(&config_text).expect("parse config");
    assert_eq!(
        document["model_reasoning_effort_selection"].as_str(),
        Some("high")
    );
    assert_eq!(
        store
            .effective_config()
            .provider
            .model_reasoning_effort_selection
            .as_deref(),
        Some("high")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn loader_reads_compaction_token_limit_from_user_config() {
    let root = unique_temp_dir("compaction-limit-load");
    let home = root.join(".devo");
    fs::create_dir_all(&home).expect("create config dir");
    fs::write(
        home.join(APP_CONFIG_FILE_NAME),
        "compaction_token_limit = 250000\n",
    )
    .expect("write config");

    let store = AppConfigStore::load(home, None).expect("load config");
    assert_eq!(
        store.effective_config().compaction_token_limit,
        Some(250_000)
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn set_compaction_token_limit_writes_user_config_and_refreshes() {
    let root = unique_temp_dir("compaction-limit-set");
    let home = root.join(".devo");
    fs::create_dir_all(&home).expect("create config dir");
    fs::write(home.join(APP_CONFIG_FILE_NAME), "theme = \"aurora\"\n").expect("write config");

    let mut store = AppConfigStore::load(home.clone(), None).expect("load config");
    store
        .set_compaction_token_limit(250_000)
        .expect("write compaction limit");

    let config_text = fs::read_to_string(home.join(APP_CONFIG_FILE_NAME)).expect("read config");
    let document: toml::Value = toml::from_str(&config_text).expect("parse config");
    assert_eq!(document["theme"].as_str(), Some("aurora"));
    assert_eq!(
        document["compaction_token_limit"].as_integer(),
        Some(250_000)
    );
    assert_eq!(
        store.effective_config().compaction_token_limit,
        Some(250_000)
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn set_compaction_token_limit_rejects_values_above_i64_max() {
    let root = unique_temp_dir("compaction-limit-overflow");
    let home = root.join(".devo");
    fs::create_dir_all(&home).expect("create config dir");

    let mut store = AppConfigStore::load(home, None).expect("load config");
    let err = store
        .set_compaction_token_limit(u64::MAX)
        .expect_err("overflow must fail");
    assert!(
        err.to_string().contains("too large"),
        "unexpected error: {err}"
    );

    let _ = fs::remove_dir_all(root);
}
