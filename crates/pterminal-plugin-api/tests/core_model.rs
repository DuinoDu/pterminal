use pterminal_plugin_api::{
    ActivationEvent, PluginLifecycleState, PluginManifest, PluginRuntime, PluginRuntimeState,
    UiMode,
};

#[test]
fn manifest_deserializes_with_expected_defaults() {
    let raw = serde_json::json!({
        "id": "acme.workspace-sidebar",
        "name": "Workspace Sidebar",
        "version": "0.1.0",
        "entry": "bin/darwin-aarch64/plugin"
    });

    let manifest: PluginManifest = serde_json::from_value(raw).expect("manifest");
    assert_eq!(manifest.runtime, PluginRuntime::Native);
    assert_eq!(manifest.ui.mode, UiMode::Data);
    assert_eq!(
        manifest.activation_events,
        vec![ActivationEvent::from("onStartupFinished")]
    );
}

#[test]
fn manifest_deserializes_contributions() {
    let raw = serde_json::json!({
        "id": "acme.workspace-sidebar",
        "name": "Workspace Sidebar",
        "version": "0.1.0",
        "entry": "bin/darwin-aarch64/plugin",
        "activationEvents": ["onCommand:acme.workspace.focus"],
        "contributes": {
            "commands": [{ "id": "acme.workspace.focus", "title": "Focus Workspace" }],
            "sidebarViews": [{ "id": "acme.workspace.tree", "title": "Workspaces", "order": 100 }],
            "tabTypes": [{ "id": "acme.browser", "title": "Browser" }]
        },
        "permissions": ["terminal.topology.read"]
    });

    let manifest: PluginManifest = serde_json::from_value(raw).expect("manifest");
    assert_eq!(manifest.contributes.commands.len(), 1);
    assert_eq!(manifest.contributes.sidebar_views.len(), 1);
    assert_eq!(manifest.contributes.tab_types.len(), 1);
    assert_eq!(manifest.permissions, vec!["terminal.topology.read"]);
}

#[test]
fn runtime_state_serializes_lifecycle() {
    let state = PluginRuntimeState {
        plugin_id: "acme.workspace-sidebar".into(),
        lifecycle: PluginLifecycleState::Failed,
        restart_count: 2,
        last_error: Some("rpc timeout".into()),
    };

    let value = serde_json::to_value(&state).expect("serialize");
    assert_eq!(value["lifecycle"], "failed");
    assert_eq!(value["restart_count"], 2);
}
