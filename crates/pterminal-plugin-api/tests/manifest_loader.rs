use std::fs;

use pterminal_plugin_api::{discover_plugin_catalog, ActivationEvent};

#[test]
fn discovers_manifests_and_builds_index_for_enabled_plugins() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();

    let enabled_dir = root.join("acme.workspace-sidebar");
    fs::create_dir_all(&enabled_dir).expect("create enabled dir");
    fs::write(
        enabled_dir.join("plugin.json"),
        serde_json::json!({
            "id": "acme.workspace-sidebar",
            "name": "Workspace Sidebar",
            "version": "0.1.0",
            "entry": "bin/darwin-aarch64/plugin",
            "activationEvents": ["onStartupFinished", "onCommand:acme.workspace.focus"]
        })
        .to_string(),
    )
    .expect("write enabled manifest");

    let disabled_dir = root.join("acme.disabled");
    fs::create_dir_all(&disabled_dir).expect("create disabled dir");
    fs::write(
        disabled_dir.join("plugin.json"),
        serde_json::json!({
            "id": "acme.disabled",
            "name": "Disabled",
            "version": "0.1.0",
            "entry": "bin/darwin-aarch64/plugin",
            "activationEvents": ["onStartupFinished"]
        })
        .to_string(),
    )
    .expect("write disabled manifest");
    fs::write(disabled_dir.join(".disabled"), "").expect("disabled marker");

    let catalog = discover_plugin_catalog(root).expect("discover");
    assert_eq!(catalog.plugins.len(), 2);
    assert!(catalog.diagnostics.is_empty());

    let startup = catalog
        .activation_index
        .get(&ActivationEvent::from("onStartupFinished"))
        .expect("startup event");
    assert_eq!(startup, &vec!["acme.workspace-sidebar".to_string()]);
}

#[test]
fn reports_invalid_manifest_diagnostics() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();

    let invalid_dir = root.join("acme.invalid");
    fs::create_dir_all(&invalid_dir).expect("create invalid dir");
    fs::write(
        invalid_dir.join("plugin.json"),
        serde_json::json!({
            "id": "",
            "name": "Invalid",
            "version": "0.1.0",
            "entry": "bin/darwin-aarch64/plugin"
        })
        .to_string(),
    )
    .expect("write invalid manifest");

    let catalog = discover_plugin_catalog(root).expect("discover");
    assert_eq!(catalog.plugins.len(), 0);
    assert_eq!(catalog.diagnostics.len(), 1);
    assert!(catalog.diagnostics[0].message.contains("id"));
}
