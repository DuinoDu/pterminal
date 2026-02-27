use std::fs;
use std::path::{Path, PathBuf};

use crate::{build_activation_index, ActivationIndex, PluginManifest};

const MANIFEST_FILE: &str = "plugin.json";
const DISABLED_MARKER: &str = ".disabled";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestDiagnostic {
    pub plugin_dir: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPlugin {
    pub root_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: PluginManifest,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PluginCatalog {
    pub plugins: Vec<DiscoveredPlugin>,
    pub activation_index: ActivationIndex,
    pub diagnostics: Vec<ManifestDiagnostic>,
}

pub fn discover_plugin_catalog(root: impl AsRef<Path>) -> std::io::Result<PluginCatalog> {
    let root = root.as_ref();
    if !root.exists() {
        return Ok(PluginCatalog::default());
    }

    let mut plugins = Vec::new();
    let mut diagnostics = Vec::new();

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let plugin_dir = entry.path();
        let manifest_path = plugin_dir.join(MANIFEST_FILE);
        if !manifest_path.is_file() {
            continue;
        }

        let raw = match fs::read_to_string(&manifest_path) {
            Ok(raw) => raw,
            Err(err) => {
                diagnostics.push(ManifestDiagnostic {
                    plugin_dir: plugin_dir.clone(),
                    message: format!("failed to read plugin.json: {err}"),
                });
                continue;
            }
        };

        let manifest: PluginManifest = match serde_json::from_str(&raw) {
            Ok(manifest) => manifest,
            Err(err) => {
                diagnostics.push(ManifestDiagnostic {
                    plugin_dir: plugin_dir.clone(),
                    message: format!("failed to parse plugin.json: {err}"),
                });
                continue;
            }
        };

        if let Err(message) = validate_manifest(&manifest) {
            diagnostics.push(ManifestDiagnostic {
                plugin_dir: plugin_dir.clone(),
                message,
            });
            continue;
        }

        let enabled = !plugin_dir.join(DISABLED_MARKER).exists();
        plugins.push(DiscoveredPlugin {
            root_dir: plugin_dir,
            manifest_path,
            manifest,
            enabled,
        });
    }

    let activation_index = build_activation_index(
        &plugins
            .iter()
            .filter(|plugin| plugin.enabled)
            .map(|plugin| plugin.manifest.clone())
            .collect::<Vec<_>>(),
    );

    Ok(PluginCatalog {
        plugins,
        activation_index,
        diagnostics,
    })
}

fn validate_manifest(manifest: &PluginManifest) -> Result<(), String> {
    if manifest.id.trim().is_empty() {
        return Err("invalid manifest: id must not be empty".to_string());
    }
    if manifest.name.trim().is_empty() {
        return Err("invalid manifest: name must not be empty".to_string());
    }
    if manifest.version.trim().is_empty() {
        return Err("invalid manifest: version must not be empty".to_string());
    }
    if manifest.entry.trim().is_empty() {
        return Err("invalid manifest: entry must not be empty".to_string());
    }
    Ok(())
}
