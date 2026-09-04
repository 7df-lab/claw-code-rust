//! Document-preserving MCP server mutations on the user `config.toml`.

use std::fs;

use crate::McpServerId;
use crate::McpServerRecord;
use crate::McpServerRecordToml;
use crate::read_provider_config_document;
use crate::write_atomic;

use super::AppConfigLoader;
use super::AppConfigStore;
use super::ensure_toml_table;

impl AppConfigStore {
    /// Upserts one MCP server record into the user-level `config.toml`.
    ///
    /// Replaces an existing server table with the same `id`, otherwise creates it.
    /// Creates the top-level `mcp_servers` table when missing.
    pub fn upsert_mcp_server(&mut self, record: McpServerRecord) -> anyhow::Result<()> {
        if record.id.0.trim().is_empty() {
            anyhow::bail!("mcp server id must not be empty");
        }

        let target_config_file = self.user_config_file.as_path();
        if let Some(parent) = target_config_file.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut document = read_provider_config_document(target_config_file)?;
        let mcp_servers = mcp_servers_table_mut(&mut document)?;
        let id = record.id.0.as_str();
        let server_value = toml::Value::try_from(McpServerRecordToml::from(&record))
            .map_err(|error| anyhow::anyhow!("failed to serialize mcp server: {error}"))?;
        mcp_servers.insert(id.to_string(), server_value);

        let data = toml::to_string_pretty(&document)?;
        write_atomic(target_config_file, data.as_bytes())?;
        self.reload_effective_config()?;
        Ok(())
    }

    /// Removes one MCP server by id from the user-level `config.toml`.
    pub fn remove_mcp_server(&mut self, id: &str) -> anyhow::Result<()> {
        let id = id.trim();
        if id.is_empty() {
            anyhow::bail!("mcp server id must not be empty");
        }

        let target_config_file = self.user_config_file.as_path();
        let mut document = read_provider_config_document(target_config_file)?;
        let mcp_servers = mcp_servers_table_mut(&mut document)?;
        if mcp_servers.remove(id).is_none() {
            anyhow::bail!("mcp server `{id}` not found");
        }

        let data = toml::to_string_pretty(&document)?;
        write_atomic(target_config_file, data.as_bytes())?;
        self.reload_effective_config()?;
        Ok(())
    }

    /// Sets the `enabled` flag for one MCP server in the user-level `config.toml`.
    ///
    /// Bundled servers (for example `code_search`) are injected into the
    /// effective config on load and may not yet exist on disk. Enabling or
    /// disabling them materializes the full bundled record into
    /// `config.toml` instead of failing with "not found".
    pub fn set_mcp_server_enabled(&mut self, id: &str, enabled: bool) -> anyhow::Result<()> {
        let id = id.trim();
        if id.is_empty() {
            anyhow::bail!("mcp server id must not be empty");
        }

        let target_config_file = self.user_config_file.as_path();
        let mut document = read_provider_config_document(target_config_file)?;
        let mcp_servers = mcp_servers_table_mut(&mut document)?;
        if !mcp_servers.contains_key(id) {
            if id == crate::BUNDLED_CODE_SEARCH_MCP_SERVER_ID {
                let mut record = crate::bundled_code_search_mcp_server();
                record.enabled = enabled;
                return self.upsert_mcp_server(record);
            }
            anyhow::bail!("mcp server `{id}` not found");
        }

        let entry = mcp_servers.get_mut(id).expect("key exists just checked");
        let table = ensure_toml_table(entry);
        table.insert("enabled".to_string(), toml::Value::Boolean(enabled));

        let data = toml::to_string_pretty(&document)?;
        write_atomic(target_config_file, data.as_bytes())?;
        self.reload_effective_config()?;
        Ok(())
    }

    /// Returns MCP servers from the effective (merged) config.
    pub fn mcp_servers(&self) -> &[McpServerRecord] {
        &self.config.mcp_runtime.servers
    }

    /// Returns the user-level config.toml path used for MCP mutations.
    pub fn user_config_file(&self) -> &std::path::Path {
        &self.user_config_file
    }

    fn reload_effective_config(&mut self) -> anyhow::Result<()> {
        self.config = self
            .loader
            .load(self.workspace_root.as_deref())
            .map_err(|error| anyhow::anyhow!(error))?;
        Ok(())
    }
}

fn mcp_servers_table_mut(
    document: &mut toml::Value,
) -> anyhow::Result<&mut toml::map::Map<String, toml::Value>> {
    let document = ensure_toml_table(document);
    let mcp_servers = document
        .entry("mcp_servers".to_string())
        .or_insert_with(|| toml::Value::Table(Default::default()));
    Ok(ensure_toml_table(mcp_servers))
}

/// Builds a default-ready MCP server record for CLI upserts.
pub fn mcp_server_record_for_cli(
    id: impl Into<String>,
    transport: crate::McpTransportConfig,
) -> McpServerRecord {
    let id = id.into();
    McpServerRecord {
        display_name: id.clone(),
        id: McpServerId(id),
        transport,
        startup_policy: crate::McpStartupPolicy::Lazy,
        enabled: true,
        trust_policy: crate::McpTrustPolicy::User,
        allowed_capabilities: Vec::new(),
        roots_policy: crate::McpRootsPolicy::None,
        output_limits: crate::McpOutputLimits::default(),
        auth_ref: None,
    }
}
