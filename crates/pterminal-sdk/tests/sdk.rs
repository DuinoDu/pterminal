use pterminal_sdk::{HostClient, InMemoryHostTransport, PluginContext};

#[test]
fn plugin_context_collects_contributions() {
    let mut ctx = PluginContext::new("acme.workspace-sidebar");
    ctx.register_command("acme.workspace.focus", "Focus Workspace");
    ctx.register_sidebar_view("acme.workspace.tree", "Workspaces", 100);
    ctx.register_tab_type("acme.browser", "Browser");

    let contributes = ctx.contributions();
    assert_eq!(contributes.commands.len(), 1);
    assert_eq!(contributes.sidebar_views.len(), 1);
    assert_eq!(contributes.tab_types.len(), 1);
}

#[test]
fn host_client_controls_runtime_via_typed_rpc() {
    let transport = InMemoryHostTransport::new(vec!["command.execute".into()]);
    let mut client = HostClient::new(transport);

    let handshake = client.handshake("1.0").expect("handshake");
    assert_eq!(handshake.protocol_version, "1.0");
    assert_eq!(handshake.host_capabilities, vec!["command.execute"]);

    client
        .activate("acme.workspace-sidebar")
        .expect("activate plugin");
    let listed = client.list_active_plugins().expect("list active");
    assert_eq!(listed, vec!["acme.workspace-sidebar"]);

    client
        .deactivate("acme.workspace-sidebar")
        .expect("deactivate plugin");
    let listed = client.list_active_plugins().expect("list after deactivate");
    assert!(listed.is_empty());
}
