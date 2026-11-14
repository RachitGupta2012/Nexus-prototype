//! Read-only MCP client foundation.
//!
//! This module manages user-configured MCP servers, connects on explicit
//! request, negotiates the protocol through the official Rust SDK, and lists
//! tool metadata. It deliberately has no execution path: no `tools/call`, no
//! resource reads, no prompt retrieval, no sampling, and no elicitation.

pub mod config;
pub mod manager;

use serde_json::Value;

pub use manager::McpManager;

/// Explicit connection lifecycle. `Ready` means metadata is available, not
/// that any tool may run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ServerState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Discovering,
    Ready,
    Degraded,
    Error,
    Disconnecting,
}

impl ServerState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Discovering => "Discovering",
            Self::Ready => "Ready",
            Self::Degraded => "Degraded",
            Self::Error => "Error",
            Self::Disconnecting => "Disconnecting",
        }
    }

    pub const fn is_connected(self) -> bool {
        matches!(
            self,
            Self::Connected | Self::Discovering | Self::Ready | Self::Degraded
        )
    }

    pub const fn is_busy(self) -> bool {
        matches!(
            self,
            Self::Connecting | Self::Discovering | Self::Disconnecting
        )
    }
}

/// Advertised server capabilities. Presence is not implementation: resources
/// and prompts are reported but never read or retrieved in this milestone.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Capabilities {
    pub tools: bool,
    pub resources: bool,
    pub prompts: bool,
    pub logging: bool,
    pub completions: bool,
    pub experimental: bool,
    pub tool_list_changed: bool,
}

impl Capabilities {
    pub fn labels(self) -> Vec<&'static str> {
        let mut labels = Vec::new();
        if self.tools {
            labels.push("Tools");
        }
        if self.resources {
            labels.push("Resources (availability only)");
        }
        if self.prompts {
            labels.push("Prompts (availability only)");
        }
        if self.logging {
            labels.push("Logging (not consumed)");
        }
        if self.completions {
            labels.push("Completions");
        }
        if self.experimental {
            labels.push("Experimental");
        }
        if self.tool_list_changed {
            labels.push("Tool list change notifications");
        }
        labels
    }
}

#[derive(Clone, Debug)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl McpTool {
    pub const fn status_label(&self) -> &'static str {
        "Discovered — execution not implemented"
    }
}

/// Read-only catalog snapshot for one server.
#[derive(Clone, Debug, Default)]
pub struct ServerStatus {
    pub state: ServerState,
    pub protocol_version: Option<String>,
    pub server_name: Option<String>,
    pub server_version: Option<String>,
    pub capabilities: Capabilities,
    pub tools: Vec<McpTool>,
    pub omitted_tools: usize,
    pub last_discovery: Option<u64>,
    pub error: Option<String>,
    /// True when metadata was retained after a disconnect or discovery failure.
    pub stale: bool,
}

impl ServerStatus {
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }
}

/// Bounds applied to untrusted server metadata before it reaches the UI.
pub const MAX_TOOLS_PER_SERVER: usize = 1_000;
pub const MAX_TOOL_NAME_BYTES: usize = 256;
pub const MAX_TOOL_DESCRIPTION_BYTES: usize = 8_192;
pub const MAX_TOOL_SCHEMA_BYTES: usize = 256 * 1_024;
pub const MAX_DISCOVERY_PAGES: usize = 10;

#[cfg(test)]
mod tests {
    #[test]
    fn discovery_surface_has_no_execution_calls() {
        let sources = [include_str!("manager.rs"), include_str!("config.rs")];
        for source in sources {
            for forbidden in [
                "call_tool",
                "read_resource",
                "get_prompt",
                "create_message",
                "create_elicitation",
                "list_resources",
                "list_prompts",
            ] {
                assert!(
                    !source.contains(forbidden),
                    "read-only MCP layer must not reference {forbidden}"
                );
            }
        }
    }
}
