use anyhow::Context;
use anyhow::Result;
use devo_core::model_reference;
use devo_core::read_provider_catalog_config;
use devo_core::write_provider_catalog_config;
use devo_protocol::PermissionPreset;
use devo_protocol::ProviderWireApi;
use devo_util_paths::find_devo_home;
use toml::Value;

pub(crate) fn save_last_used_model(
    wire_api: Option<ProviderWireApi>,
    provider: ProviderWireApi,
    model: &str,
) -> Result<()> {
    let path = find_devo_home()
        .context("could not determine user config path")?
        .join(devo_core::PROVIDER_CONFIG_FILE_NAME);
    let mut config = read_provider_catalog_config(&path).map_err(anyhow::Error::msg)?;
    let model_ref = if model.contains('/') {
        model.to_string()
    } else if let Some((provider_id, _)) = config
        .providers
        .iter()
        .find(|(_, provider_config)| provider_config.models.contains_key(model))
    {
        model_reference(provider_id, model)
    } else {
        let provider_id = wire_api
            .or(Some(provider))
            .map(|wire_api| wire_api.as_str())
            .unwrap_or("default");
        model_reference(provider_id, model)
    };
    config.model = Some(model_ref);
    write_provider_catalog_config(&path, &config).map_err(anyhow::Error::msg)?;
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn save_reasoning_effort_selection(selection: Option<&str>) -> Result<()> {
    let path = find_devo_home()
        .context("could not determine user config path")?
        .join(devo_core::PROVIDER_CONFIG_FILE_NAME);
    let mut config = read_provider_catalog_config(&path).map_err(anyhow::Error::msg)?;
    config.reasoning_effort = selection.map(str::to_string);
    write_provider_catalog_config(&path, &config).map_err(anyhow::Error::msg)?;
    Ok(())
}

pub(crate) fn save_default_collaboration_mode(
    mode: devo_protocol::CollaborationMode,
) -> Result<()> {
    let config_home = find_devo_home().context("could not determine user config path")?;
    let mut store = devo_core::AppConfigStore::load(config_home, /*workspace_root*/ None)
        .map_err(|error| anyhow::anyhow!(error))?;
    store
        .set_default_collaboration_mode(mode)
        .context("failed to persist default_collaboration_mode")?;
    Ok(())
}

pub(crate) fn save_theme_selection(name: &str) -> Result<()> {
    let path = find_devo_home()
        .context("could not determine user config path")?
        .join("config.toml");
    let mut root = if path.exists() {
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        data.parse::<Value>()
            .with_context(|| format!("failed to parse {}", path.display()))?
    } else {
        Value::Table(Default::default())
    };
    root = merge_theme_selection(root, name)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let rendered = toml::to_string_pretty(&root)?;

    std::fs::write(&path, rendered)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub(crate) fn save_project_permission_preset(
    project_key: &str,
    preset: PermissionPreset,
) -> Result<()> {
    save_project_config_value(
        project_key,
        "permission_preset",
        permission_preset_to_config_value(preset),
    )
}

pub(crate) fn save_project_sandbox_profile(project_key: &str, profile: &str) -> Result<()> {
    save_project_config_value(project_key, "sandbox_profile", profile)
}

fn save_project_config_value(project_key: &str, key: &str, value: &str) -> Result<()> {
    let path = find_devo_home()
        .context("could not determine user config path")?
        .join("config.toml");
    let mut root = if path.exists() {
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        data.parse::<Value>()
            .with_context(|| format!("failed to parse {}", path.display()))?
    } else {
        Value::Table(Default::default())
    };
    root = merge_project_config_value(root, project_key, key, value)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let rendered = toml::to_string_pretty(&root)?;

    std::fs::write(&path, rendered)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub(crate) fn load_theme_selection() -> Option<String> {
    let path = find_devo_home().ok()?.join("config.toml");
    let data = std::fs::read_to_string(&path).ok()?;
    let root: Value = data.parse().ok()?;
    root.get("theme")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

pub(crate) fn save_collapse_reasoning(collapsed: bool) -> Result<()> {
    let path = find_devo_home()
        .context("could not determine user config path")?
        .join("config.toml");
    let mut root = if path.exists() {
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        data.parse::<Value>()
            .with_context(|| format!("failed to parse {}", path.display()))?
    } else {
        Value::Table(Default::default())
    };
    root = merge_collapse_reasoning(root, collapsed)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let rendered = toml::to_string_pretty(&root)?;

    std::fs::write(&path, rendered)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub(crate) fn load_collapse_reasoning() -> bool {
    let path = find_devo_home().ok().map(|home| home.join("config.toml"));
    let Some(path) = path else {
        return true;
    };
    let Ok(data) = std::fs::read_to_string(&path) else {
        return true;
    };
    let Ok(root) = data.parse::<Value>() else {
        return true;
    };
    root.get("collapse_reasoning")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn merge_project_config_value(
    mut root: Value,
    project_key: &str,
    key: &str,
    value: &str,
) -> Result<Value> {
    let table = root
        .as_table_mut()
        .context("config root must be a TOML table")?;
    let projects = table
        .entry("projects".to_string())
        .or_insert_with(|| Value::Table(Default::default()));
    let projects_table = projects
        .as_table_mut()
        .context("projects must be a TOML table")?;
    let project = projects_table
        .entry(project_key.to_string())
        .or_insert_with(|| Value::Table(Default::default()));
    let project_table = project
        .as_table_mut()
        .context("project permission entry must be a TOML table")?;
    project_table.insert(key.to_string(), Value::String(value.to_string()));
    Ok(root)
}

fn merge_theme_selection(mut root: Value, name: &str) -> Result<Value> {
    let table = root
        .as_table_mut()
        .context("config root must be a TOML table")?;
    table.insert("theme".to_string(), Value::String(name.to_string()));
    Ok(root)
}

fn merge_collapse_reasoning(mut root: Value, collapsed: bool) -> Result<Value> {
    let table = root
        .as_table_mut()
        .context("config root must be a TOML table")?;
    table.insert("collapse_reasoning".to_string(), Value::Boolean(collapsed));
    Ok(root)
}

fn permission_preset_to_config_value(preset: PermissionPreset) -> &'static str {
    match preset {
        PermissionPreset::Default => "default",
        PermissionPreset::AutoReview => "auto-review",
        PermissionPreset::FullAccess => "full-access",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn merge_project_permission_preset_preserves_unrelated_config() {
        let root: Value = r#"
model = "gpt-5.4"

[projects.old]
permission_preset = "default"
custom = "keep"
"#
        .parse()
        .expect("parse");

        let merged =
            merge_project_config_value(root, "C:\\repo", "permission_preset", "full-access")
                .expect("merge");

        assert_eq!(
            merged
                .as_table()
                .and_then(|table| table.get("model"))
                .and_then(Value::as_str),
            Some("gpt-5.4")
        );
        assert_eq!(
            merged
                .as_table()
                .and_then(|table| table.get("projects"))
                .and_then(Value::as_table)
                .and_then(|projects| projects.get("old"))
                .and_then(Value::as_table)
                .and_then(|project| project.get("permission_preset"))
                .and_then(Value::as_str),
            Some("default")
        );
        assert_eq!(
            merged
                .as_table()
                .and_then(|table| table.get("projects"))
                .and_then(Value::as_table)
                .and_then(|projects| projects.get("old"))
                .and_then(Value::as_table)
                .and_then(|project| project.get("custom"))
                .and_then(Value::as_str),
            Some("keep")
        );
        assert_eq!(
            merged
                .as_table()
                .and_then(|table| table.get("projects"))
                .and_then(Value::as_table)
                .and_then(|projects| projects.get("C:\\repo"))
                .and_then(Value::as_table)
                .and_then(|project| project.get("permission_preset"))
                .and_then(Value::as_str),
            Some("full-access")
        );
    }

    #[test]
    fn merge_project_sandbox_profile_preserves_other_project_keys() {
        let root: Value = r#"
[projects.old]
permission_preset = "default"
sandbox_profile = "strict"
"#
        .parse()
        .expect("parse");

        let merged = merge_project_config_value(root, "C:\\repo", "sandbox_profile", "read-only")
            .expect("merge");

        assert_eq!(
            merged
                .as_table()
                .and_then(|table| table.get("projects"))
                .and_then(Value::as_table)
                .and_then(|projects| projects.get("old"))
                .and_then(Value::as_table)
                .and_then(|project| project.get("sandbox_profile"))
                .and_then(Value::as_str),
            Some("strict")
        );
        assert_eq!(
            merged
                .as_table()
                .and_then(|table| table.get("projects"))
                .and_then(Value::as_table)
                .and_then(|projects| projects.get("C:\\repo"))
                .and_then(Value::as_table)
                .and_then(|project| project.get("sandbox_profile"))
                .and_then(Value::as_str),
            Some("read-only")
        );
    }
}
