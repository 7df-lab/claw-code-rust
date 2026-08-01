//! Document-preserving MCP server mutations on the user `config.toml`.

use std::fs;

use crate::McpServerId;
use crate::McpServerRecord;
use crate::read_provider_config_document;
use crate::write_atomic;

use super::AppConfigLoader;
use super::AppConfigStore;
use super::ensure_toml_table;

impl AppConfigStore {
    /// Upserts one MCP server record into the user-level `config.toml`.
    ///
    /// Replaces an existing entry with the same `id`, otherwise appends. Creates
    /// the `[mcp]` table and `servers` array when missing.
    pub fn upsert_mcp_server(&mut self, record: McpServerRecord) -> anyhow::Result<()> {
        if record.id.0.trim().is_empty() {
            anyhow::bail!("mcp server id must not be empty");
        }

        let target_config_file = self.user_config_file.as_path();
        if let Some(parent) = target_config_file.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut document = read_provider_config_document(target_config_file)?;
        let servers = mcp_servers_array_mut(&mut document)?;
        let server_value = toml::Value::try_from(&record)
            .map_err(|error| anyhow::anyhow!("failed to serialize mcp server: {error}"))?;
        let id = record.id.0.as_str();
        if let Some(existing) = servers
            .iter_mut()
            .find(|entry| server_entry_id(entry) == Some(id))
        {
            *existing = server_value;
        } else {
            servers.push(server_value);
        }

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
        let servers = mcp_servers_array_mut(&mut document)?;
        let before = servers.len();
        servers.retain(|entry| server_entry_id(entry) != Some(id));
        if servers.len() == before {
            anyhow::bail!("mcp server `{id}` not found");
        }

        let data = toml::to_string_pretty(&document)?;
        write_atomic(target_config_file, data.as_bytes())?;
        self.reload_effective_config()?;
        Ok(())
    }

    /// Sets the `enabled` flag for one MCP server in the user-level `config.toml`.
    pub fn set_mcp_server_enabled(&mut self, id: &str, enabled: bool) -> anyhow::Result<()> {
        let id = id.trim();
        if id.is_empty() {
            anyhow::bail!("mcp server id must not be empty");
        }

        let target_config_file = self.user_config_file.as_path();
        let mut document = read_provider_config_document(target_config_file)?;
        let servers = mcp_servers_array_mut(&mut document)?;
        let Some(entry) = servers
            .iter_mut()
            .find(|entry| server_entry_id(entry) == Some(id))
        else {
            anyhow::bail!("mcp server `{id}` not found");
        };
        let table = ensure_toml_table(entry);
        table.insert("enabled".to_string(), toml::Value::Boolean(enabled));

        let data = toml::to_string_pretty(&document)?;
        write_atomic(target_config_file, data.as_bytes())?;
        self.reload_effective_config()?;
        Ok(())
    }

    /// Returns MCP servers from the effective (merged) config.
    pub fn mcp_servers(&self) -> &[McpServerRecord] {
        &self.config.mcp.servers
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

fn mcp_servers_array_mut(document: &mut toml::Value) -> anyhow::Result<&mut Vec<toml::Value>> {
    let document = ensure_toml_table(document);
    let mcp = document
        .entry("mcp".to_string())
        .or_insert_with(|| toml::Value::Table(Default::default()));
    let mcp = ensure_toml_table(mcp);
    let servers = mcp
        .entry("servers".to_string())
        .or_insert_with(|| toml::Value::Array(Vec::new()));
    if !servers.is_array() {
        *servers = toml::Value::Array(Vec::new());
    }
    servers
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("mcp.servers must be an array"))
}

fn server_entry_id(entry: &toml::Value) -> Option<&str> {
    entry
        .as_table()
        .and_then(|table| table.get("id"))
        .and_then(toml::Value::as_str)
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
