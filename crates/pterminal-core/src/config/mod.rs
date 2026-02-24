pub mod theme;

use std::path::PathBuf;

use anyhow::Result;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

pub use theme::Theme;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: GeneralConfig,
    pub font: FontConfig,
    pub theme: ThemeRef,
    pub window: WindowConfig,
    pub scrollback: ScrollbackConfig,
    pub cursor: CursorConfig,
    pub sidebar: SidebarConfig,
    pub notification: NotificationConfig,
    pub tmux: TmuxConfig,
    pub keybindings: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub shell: String,
    pub working_directory: String,
    pub confirm_close_process: bool,
    pub new_workspace_placement: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
    pub bold_is_bright: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeRef {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    pub opacity: f32,
    pub blur: bool,
    pub decorations: String,
    pub startup_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrollbackConfig {
    pub lines: usize,
    pub multiplier: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorConfig {
    pub style: String,
    pub blink: bool,
    pub blink_interval_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarConfig {
    pub width: u32,
    pub show_git_branch: bool,
    pub show_cwd: bool,
    pub show_ports: bool,
    pub show_notification_badge: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationConfig {
    pub enabled: bool,
    pub detect_bell: bool,
    pub detect_osc: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TmuxConfig {
    pub detect: bool,
    pub passthrough_hint: bool,
    pub prefer_socket_notify: bool,
}

impl Config {
    /// Load config from default path (~/.config/pterminal/config.toml)
    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Config::default())
        }
    }

    pub fn config_dir() -> PathBuf {
        ProjectDirs::from("", "", "pterminal")
            .map(|d| d.config_dir().to_path_buf())
            .unwrap_or_else(|| dirs_fallback().join("pterminal"))
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    /// Resolve the shell to use
    pub fn shell(&self) -> String {
        if !self.general.shell.is_empty() {
            return self.general.shell.clone();
        }
        std::env::var("SHELL").unwrap_or_else(|_| {
            if cfg!(windows) {
                "powershell.exe".to_string()
            } else {
                "/bin/sh".to_string()
            }
        })
    }

    /// Resolve the working directory
    pub fn working_directory(&self) -> PathBuf {
        if !self.general.working_directory.is_empty() {
            return PathBuf::from(&self.general.working_directory);
        }
        dirs_fallback()
    }
}

fn dirs_fallback() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            font: FontConfig::default(),
            theme: ThemeRef::default(),
            window: WindowConfig::default(),
            scrollback: ScrollbackConfig::default(),
            cursor: CursorConfig::default(),
            sidebar: SidebarConfig::default(),
            notification: NotificationConfig::default(),
            tmux: TmuxConfig::default(),
            keybindings: default_keybindings(),
        }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            shell: String::new(),
            working_directory: String::new(),
            confirm_close_process: true,
            new_workspace_placement: "after-current".to_string(),
        }
    }
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "Monaco".to_string(),
            size: 14.0,
            bold_is_bright: false,
        }
    }
}

impl Default for ThemeRef {
    fn default() -> Self {
        Self {
            name: "default-dark".to_string(),
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            blur: false,
            decorations: "full".to_string(),
            startup_mode: "windowed".to_string(),
        }
    }
}

impl Default for ScrollbackConfig {
    fn default() -> Self {
        Self {
            lines: 10_000,
            multiplier: 3,
        }
    }
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            style: "block".to_string(),
            blink: true,
            blink_interval_ms: 530,
        }
    }
}

impl Default for SidebarConfig {
    fn default() -> Self {
        Self {
            width: 220,
            show_git_branch: true,
            show_cwd: true,
            show_ports: true,
            show_notification_badge: true,
        }
    }
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            detect_bell: true,
            detect_osc: true,
        }
    }
}

impl Default for TmuxConfig {
    fn default() -> Self {
        Self {
            detect: true,
            passthrough_hint: true,
            prefer_socket_notify: true,
        }
    }
}

fn default_keybindings() -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    m.insert("ctrl+shift+t".into(), "new-workspace".into());
    m.insert("ctrl+shift+w".into(), "close-workspace".into());
    m.insert("ctrl+shift+d".into(), "split-right".into());
    m.insert("ctrl+shift+e".into(), "split-down".into());
    m.insert("ctrl+shift+h".into(), "focus-left".into());
    m.insert("ctrl+shift+l".into(), "focus-right".into());
    m.insert("ctrl+shift+j".into(), "focus-down".into());
    m.insert("ctrl+shift+k".into(), "focus-up".into());
    m.insert("ctrl+shift+p".into(), "command-palette".into());
    m.insert("ctrl+shift+f".into(), "search".into());
    m.insert("ctrl+shift+n".into(), "notifications".into());
    m.insert("ctrl+tab".into(), "next-workspace".into());
    m.insert("ctrl+shift+tab".into(), "prev-workspace".into());
    m
}
