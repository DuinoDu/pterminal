use anyhow::{anyhow, Result};
use pterminal_plugin_api::{
    CommandContribution, Contributions, PaneContentSnapshot, PaneStateSnapshot, SidebarViewContribution,
    TabTypeContribution, TerminalTopology,
};
use pterminal_plugin_host::{
    HostRequest, HostRequestPayload, HostResponse, HostResponsePayload, PluginHostRuntime,
};
use std::collections::HashSet;

pub trait Plugin {
    fn activate(&mut self, ctx: &mut PluginContext) -> Result<()>;

    fn deactivate(&mut self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginContext {
    plugin_id: String,
    contributes: Contributions,
}

impl PluginContext {
    pub fn new(plugin_id: impl Into<String>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            contributes: Contributions::default(),
        }
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn register_command(&mut self, id: impl Into<String>, title: impl Into<String>) {
        self.contributes.commands.push(CommandContribution {
            id: id.into(),
            title: title.into(),
        });
    }

    pub fn register_sidebar_view(
        &mut self,
        id: impl Into<String>,
        title: impl Into<String>,
        order: i32,
    ) {
        self.contributes
            .sidebar_views
            .push(SidebarViewContribution {
                id: id.into(),
                title: title.into(),
                icon: None,
                order,
            });
    }

    pub fn register_tab_type(&mut self, id: impl Into<String>, title: impl Into<String>) {
        self.contributes.tab_types.push(TabTypeContribution {
            id: id.into(),
            title: title.into(),
        });
    }

    pub fn contributions(&self) -> &Contributions {
        &self.contributes
    }
}

pub trait HostTransport {
    fn request(&mut self, request: HostRequest) -> Result<HostResponse>;
}

pub struct InMemoryHostTransport {
    runtime: PluginHostRuntime,
}

impl InMemoryHostTransport {
    pub fn new(host_capabilities: Vec<String>) -> Self {
        Self {
            runtime: PluginHostRuntime::new(host_capabilities),
        }
    }
}

impl HostTransport for InMemoryHostTransport {
    fn request(&mut self, request: HostRequest) -> Result<HostResponse> {
        Ok(self.runtime.handle(request))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeInfo {
    pub protocol_version: String,
    pub host_capabilities: Vec<String>,
}

pub struct HostClient<T: HostTransport> {
    transport: T,
    next_id: u64,
}

impl<T: HostTransport> HostClient<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            next_id: 1,
        }
    }

    pub fn handshake(&mut self, protocol_version: &str) -> Result<HandshakeInfo> {
        let payload = self.call(HostRequestPayload::Handshake {
            protocol_version: protocol_version.to_string(),
            host_capabilities: Vec::new(),
        })?;
        match payload {
            HostResponsePayload::HandshakeAck {
                protocol_version,
                host_capabilities,
            } => Ok(HandshakeInfo {
                protocol_version,
                host_capabilities,
            }),
            other => Err(anyhow!("unexpected handshake response: {other:?}")),
        }
    }

    pub fn activate(&mut self, plugin_id: &str) -> Result<()> {
        let payload = self.call(HostRequestPayload::Activate {
            plugin_id: plugin_id.to_string(),
        })?;
        match payload {
            HostResponsePayload::Activated { .. } => Ok(()),
            HostResponsePayload::Error { message } => Err(anyhow!(message)),
            other => Err(anyhow!("unexpected activate response: {other:?}")),
        }
    }

    pub fn deactivate(&mut self, plugin_id: &str) -> Result<()> {
        let payload = self.call(HostRequestPayload::Deactivate {
            plugin_id: plugin_id.to_string(),
        })?;
        match payload {
            HostResponsePayload::Deactivated { .. } => Ok(()),
            HostResponsePayload::Error { message } => Err(anyhow!(message)),
            other => Err(anyhow!("unexpected deactivate response: {other:?}")),
        }
    }

    pub fn list_active_plugins(&mut self) -> Result<Vec<String>> {
        let payload = self.call(HostRequestPayload::ListActivePlugins)?;
        match payload {
            HostResponsePayload::ActivePlugins { plugin_ids } => Ok(plugin_ids),
            HostResponsePayload::Error { message } => Err(anyhow!(message)),
            other => Err(anyhow!("unexpected list response: {other:?}")),
        }
    }

    fn call(&mut self, payload: HostRequestPayload) -> Result<HostResponsePayload> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let response = self.transport.request(HostRequest { id, payload })?;
        if response.id != id {
            return Err(anyhow!(
                "mismatched response id: expected {id}, got {}",
                response.id
            ));
        }
        Ok(response.payload)
    }
}

pub trait TerminalSnapshotProvider {
    fn topology(&self) -> Result<TerminalTopology>;
    fn pane_states(&self) -> Result<Vec<PaneStateSnapshot>>;
    fn pane_content(&self, pane_id: u64, max_lines: usize) -> Result<PaneContentSnapshot>;
}

pub struct TerminalIntrospectionApi<P: TerminalSnapshotProvider> {
    provider: P,
    permissions: HashSet<String>,
    max_content_reads: u32,
    content_reads: u32,
}

impl<P: TerminalSnapshotProvider> TerminalIntrospectionApi<P> {
    pub fn new(provider: P, permissions: Vec<String>, max_content_reads: u32) -> Self {
        Self {
            provider,
            permissions: permissions.into_iter().collect(),
            max_content_reads: max_content_reads.max(1),
            content_reads: 0,
        }
    }

    pub fn topology(&mut self) -> Result<TerminalTopology> {
        self.require_permission("terminal.topology.read")?;
        self.provider.topology()
    }

    pub fn pane_states(&mut self) -> Result<Vec<PaneStateSnapshot>> {
        self.require_permission("terminal.pane.state.read")?;
        self.provider.pane_states()
    }

    pub fn pane_content(&mut self, pane_id: u64, max_lines: usize) -> Result<PaneContentSnapshot> {
        self.require_permission("terminal.pane.content.read")?;
        if self.content_reads >= self.max_content_reads {
            return Err(anyhow!("pane content read rate limit exceeded"));
        }
        self.content_reads = self.content_reads.saturating_add(1);
        let bounded_lines = max_lines.clamp(1, 2000);
        self.provider.pane_content(pane_id, bounded_lines)
    }

    fn require_permission(&self, permission: &str) -> Result<()> {
        if self.permissions.contains(permission) {
            return Ok(());
        }
        Err(anyhow!("missing required permission: {permission}"))
    }
}
