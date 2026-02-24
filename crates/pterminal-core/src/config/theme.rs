use serde::{Deserialize, Serialize};

/// Terminal color theme
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub colors: ThemeColors,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeColors {
    pub background: RgbColor,
    pub foreground: RgbColor,
    pub cursor: RgbColor,
    pub selection_bg: RgbColor,
    pub selection_fg: RgbColor,
    /// ANSI colors 0-15
    pub ansi: [RgbColor; 16],
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RgbColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn to_wgpu_color(self) -> [f32; 4] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            1.0,
        ]
    }

    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.trim_start_matches('#');
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some(Self { r, g, b })
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            name: "default-dark".to_string(),
            colors: ThemeColors::default(),
        }
    }
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self {
            background: RgbColor::new(30, 30, 46),    // #1e1e2e (Catppuccin Mocha)
            foreground: RgbColor::new(205, 214, 244),  // #cdd6f4
            cursor: RgbColor::new(245, 224, 220),      // #f5e0dc
            selection_bg: RgbColor::new(88, 91, 112),   // #585b70
            selection_fg: RgbColor::new(205, 214, 244), // #cdd6f4
            ansi: [
                // Normal colors (0-7)
                RgbColor::new(69, 71, 90),     // black
                RgbColor::new(243, 139, 168),  // red
                RgbColor::new(166, 227, 161),  // green
                RgbColor::new(249, 226, 175),  // yellow
                RgbColor::new(137, 180, 250),  // blue
                RgbColor::new(245, 194, 231),  // magenta
                RgbColor::new(148, 226, 213),  // cyan
                RgbColor::new(186, 194, 222),  // white
                // Bright colors (8-15)
                RgbColor::new(88, 91, 112),    // bright black
                RgbColor::new(243, 139, 168),  // bright red
                RgbColor::new(166, 227, 161),  // bright green
                RgbColor::new(249, 226, 175),  // bright yellow
                RgbColor::new(137, 180, 250),  // bright blue
                RgbColor::new(245, 194, 231),  // bright magenta
                RgbColor::new(148, 226, 213),  // bright cyan
                RgbColor::new(205, 214, 244),  // bright white
            ],
        }
    }
}
