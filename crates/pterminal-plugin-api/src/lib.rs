use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub type PluginId = String;
pub type ActivationIndex = BTreeMap<ActivationEvent, Vec<PluginId>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub id: PluginId,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub sdk: SdkManifest,
    #[serde(default)]
    pub runtime: PluginRuntime,
    pub entry: String,
    #[serde(default)]
    pub ui: UiManifest,
    #[serde(default = "default_activation_events")]
    pub activation_events: Vec<ActivationEvent>,
    #[serde(default)]
    pub contributes: Contributions,
    #[serde(default)]
    pub permissions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkManifest {
    #[serde(default = "default_sdk_version")]
    pub version: String,
}

impl Default for SdkManifest {
    fn default() -> Self {
        Self {
            version: default_sdk_version(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PluginRuntime {
    #[default]
    Native,
    Node,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum UiMode {
    #[default]
    Data,
    SlintSandbox,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiManifest {
    #[serde(default)]
    pub mode: UiMode,
}

impl Default for UiManifest {
    fn default() -> Self {
        Self { mode: UiMode::Data }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ActivationEvent(pub String);

impl From<&str> for ActivationEvent {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for ActivationEvent {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Contributions {
    #[serde(default)]
    pub commands: Vec<CommandContribution>,
    #[serde(default)]
    pub sidebar_views: Vec<SidebarViewContribution>,
    #[serde(default)]
    pub tab_types: Vec<TabTypeContribution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandContribution {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidebarViewContribution {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub order: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabTypeContribution {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PluginLifecycleState {
    #[default]
    Discovered,
    Loaded,
    Active,
    Failed,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRuntimeState {
    pub plugin_id: PluginId,
    pub lifecycle: PluginLifecycleState,
    #[serde(default)]
    pub restart_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

pub fn build_activation_index(manifests: &[PluginManifest]) -> ActivationIndex {
    let mut index: ActivationIndex = BTreeMap::new();
    for manifest in manifests {
        for event in &manifest.activation_events {
            index
                .entry(event.clone())
                .or_default()
                .push(manifest.id.clone());
        }
    }
    index
}

fn default_sdk_version() -> String {
    "1.x".to_string()
}

fn default_activation_events() -> Vec<ActivationEvent> {
    vec![ActivationEvent::from("onStartupFinished")]
}
