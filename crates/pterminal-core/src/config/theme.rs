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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
            name: "iterm2-default".to_string(),
            colors: ThemeColors::default(),
        }
    }
}

impl Default for ThemeColors {
    fn default() -> Self {
        // iTerm2 default dark theme (Snazzy variant)
        Self {
            background: RgbColor::new(0x27, 0x29, 0x35),  // #272935
            foreground: RgbColor::new(0xef, 0xf0, 0xea),  // #eff0ea
            cursor: RgbColor::new(0xe9, 0xe9, 0xe9),      // #e9e9e9
            selection_bg: RgbColor::new(0x92, 0xbb, 0xd0),  // #92bbd0
            selection_fg: RgbColor::new(0x00, 0x00, 0x00),  // #000000
            ansi: [
                // Normal colors (0-7)
                RgbColor::new(0x00, 0x00, 0x00), // 0 black    #000000
                RgbColor::new(0xff, 0x5b, 0x56), // 1 red      #ff5b56
                RgbColor::new(0x5a, 0xf7, 0x8d), // 2 green    #5af78d
                RgbColor::new(0xf3, 0xf9, 0x9c), // 3 yellow   #f3f99c
                RgbColor::new(0x57, 0xc7, 0xfe), // 4 blue     #57c7fe
                RgbColor::new(0xff, 0x69, 0xc0), // 5 magenta  #ff69c0
                RgbColor::new(0x9a, 0xec, 0xfe), // 6 cyan     #9aecfe
                RgbColor::new(0xf1, 0xf1, 0xf0), // 7 white    #f1f1f0
                // Bright colors (8-15)
                RgbColor::new(0x68, 0x67, 0x67), // 8  bright black   #686767
                RgbColor::new(0xff, 0x5b, 0x56), // 9  bright red     #ff5b56
                RgbColor::new(0x5a, 0xf7, 0x8d), // 10 bright green   #5af78d
                RgbColor::new(0xf3, 0xf9, 0x9c), // 11 bright yellow  #f3f99c
                RgbColor::new(0x57, 0xc7, 0xfe), // 12 bright blue    #57c7fe
                RgbColor::new(0xff, 0x69, 0xc0), // 13 bright magenta #ff69c0
                RgbColor::new(0x9a, 0xec, 0xfe), // 14 bright cyan    #9aecfe
                RgbColor::new(0xf1, 0xf1, 0xf0), // 15 bright white   #f1f1f0
            ],
        }
    }
}
