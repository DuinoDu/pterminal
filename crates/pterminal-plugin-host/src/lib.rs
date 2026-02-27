use std::collections::BTreeSet;

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRequest {
    pub id: u64,
    pub payload: HostRequestPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostRequestPayload {
    Handshake {
        protocol_version: String,
        host_capabilities: Vec<String>,
    },
    Activate {
        plugin_id: String,
    },
    Deactivate {
        plugin_id: String,
    },
    Reload {
        plugin_id: String,
    },
    ListActivePlugins,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostResponse {
    pub id: u64,
    pub payload: HostResponsePayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostResponsePayload {
    HandshakeAck {
        protocol_version: String,
        host_capabilities: Vec<String>,
    },
    Activated {
        plugin_id: String,
    },
    Deactivated {
        plugin_id: String,
    },
    Reloaded {
        plugin_id: String,
    },
    ActivePlugins {
        plugin_ids: Vec<String>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Default)]
pub struct PluginHostRuntime {
    protocol_version: String,
    host_capabilities: Vec<String>,
    active_plugins: BTreeSet<String>,
}

impl PluginHostRuntime {
    pub fn new(host_capabilities: Vec<String>) -> Self {
        Self {
            protocol_version: "1.0".to_string(),
            host_capabilities,
            active_plugins: BTreeSet::new(),
        }
    }

    pub fn handle(&mut self, request: HostRequest) -> HostResponse {
        let payload = match request.payload {
            HostRequestPayload::Handshake { .. } => HostResponsePayload::HandshakeAck {
                protocol_version: self.protocol_version.clone(),
                host_capabilities: self.host_capabilities.clone(),
            },
            HostRequestPayload::Activate { plugin_id } => {
                self.active_plugins.insert(plugin_id.clone());
                HostResponsePayload::Activated { plugin_id }
            }
            HostRequestPayload::Deactivate { plugin_id } => {
                self.active_plugins.remove(&plugin_id);
                HostResponsePayload::Deactivated { plugin_id }
            }
            HostRequestPayload::Reload { plugin_id } => {
                if self.active_plugins.contains(&plugin_id) {
                    HostResponsePayload::Reloaded { plugin_id }
                } else {
                    HostResponsePayload::Error {
                        message: format!("plugin not active: {plugin_id}"),
                    }
                }
            }
            HostRequestPayload::ListActivePlugins => HostResponsePayload::ActivePlugins {
                plugin_ids: self.active_plugins.iter().cloned().collect(),
            },
        };

        HostResponse {
            id: request.id,
            payload,
        }
    }

    pub fn handle_json_line(&mut self, raw: &str) -> anyhow::Result<String> {
        let request: HostRequest =
            serde_json::from_str(raw).with_context(|| format!("failed to decode request: {raw}"))?;
        let response = self.handle(request);
        serde_json::to_string(&response).context("failed to encode response")
    }
}
