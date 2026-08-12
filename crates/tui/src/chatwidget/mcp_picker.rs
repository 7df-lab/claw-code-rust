//! Interactive `/mcps` picker flow on `ChatWidget`.

use std::path::PathBuf;

use crate::bottom_pane::list_selection_view::ListSelectionView;
use crate::mcp_picker::McpPickerServer;
use crate::mcp_picker::mcp_server_detail_params;
use crate::mcp_picker::mcp_server_list_params;
use crate::mcp_picker::mcp_tools_list_params;
use crate::mcp_picker::merge_mcp_picker_servers;
use devo_core::AppConfigLoader;
use devo_core::FileSystemAppConfigLoader;
use devo_core::McpConfig;
use devo_protocol::native::rpc_admin::McpServerInfo;
use devo_protocol::native::rpc_admin::McpToolEntry;
use devo_util_paths::find_devo_home;

use super::ChatWidget;

impl ChatWidget {
    pub(crate) fn set_mcp_reopen_detail(&mut self, name: Option<String>) {
        self.mcp_reopen_detail = name;
    }

    pub(super) fn on_mcp_servers_listed(&mut self, runtime: Vec<McpServerInfo>) {
        let (config, config_path) = load_mcp_config_for_picker(Some(&self.session.cwd));
        let servers = merge_mcp_picker_servers(&config, &runtime, &config_path);
        self.mcp_servers_snapshot = Some(servers);

        if let Some(name) = self.mcp_reopen_detail.take() {
            self.open_mcp_server_detail(&name);
            return;
        }

        self.open_mcp_server_list();
    }

    pub(super) fn on_mcp_tools_listed(&mut self, name: String, tools: Vec<McpToolEntry>) {
        self.open_mcp_tools_list(&name, &tools);
    }

    pub(super) fn open_mcp_server_list(&mut self) {
        let Some(servers) = self.mcp_servers_snapshot.clone() else {
            self.set_status_message("No MCP server snapshot");
            return;
        };
        if servers.is_empty() {
            self.set_status_message("No MCP servers configured");
        } else {
            self.set_status_message("Select an MCP server");
        }
        self.bottom_pane
            .open_popup_view(Box::new(ListSelectionView::new(
                mcp_server_list_params(&servers),
                self.app_event_tx.clone(),
                self.active_accent_color(),
            )));
        self.frame_requester.schedule_frame();
    }

    pub(super) fn open_mcp_server_detail(&mut self, name: &str) {
        let Some(server) = self
            .mcp_servers_snapshot
            .as_ref()
            .and_then(|servers| servers.iter().find(|server| server.id == name))
            .cloned()
        else {
            self.set_status_message(format!("MCP server `{name}` not found"));
            self.open_mcp_server_list();
            return;
        };
        self.open_mcp_server_detail_view(server);
    }

    fn open_mcp_server_detail_view(&mut self, server: McpPickerServer) {
        self.set_status_message(format!("MCP · {}", server.display_name));
        self.bottom_pane
            .open_popup_view(Box::new(ListSelectionView::new(
                mcp_server_detail_params(&server),
                self.app_event_tx.clone(),
                self.active_accent_color(),
            )));
        self.frame_requester.schedule_frame();
    }

    fn open_mcp_tools_list(&mut self, name: &str, tools: &[McpToolEntry]) {
        self.set_status_message(format!("Tools · {name}"));
        self.bottom_pane
            .open_popup_view(Box::new(ListSelectionView::new(
                mcp_tools_list_params(name, tools),
                self.app_event_tx.clone(),
                self.active_accent_color(),
            )));
        self.frame_requester.schedule_frame();
    }
}

fn load_mcp_config_for_picker(cwd: Option<&std::path::Path>) -> (McpConfig, PathBuf) {
    let config_home = find_devo_home().unwrap_or_else(|_| PathBuf::from("."));
    let config_path = config_home.join("config.toml");
    let config = FileSystemAppConfigLoader::new(config_home)
        .load(cwd)
        .map(|app| app.mcp)
        .unwrap_or_default();
    (config, config_path)
}
