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
            name: "monokai".to_string(),
            colors: ThemeColors::default(),
        }
    }
}

impl Default for ThemeColors {
    fn default() -> Self {
        // Monokai theme — matches cmux
        Self {
            background: RgbColor::new(0x27, 0x28, 0x22),  // #272822
            foreground: RgbColor::new(0xfd, 0xff, 0xf1),  // #fdfff1
            cursor: RgbColor::new(0xc0, 0xc1, 0xb5),      // #c0c1b5
            selection_bg: RgbColor::new(0x57, 0x58, 0x4f),  // #57584f
            selection_fg: RgbColor::new(0xfd, 0xff, 0xf1),  // #fdfff1
            ansi: [
                // Normal colors (0-7)
                RgbColor::new(0x27, 0x28, 0x22), // 0 black    #272822
                RgbColor::new(0xf9, 0x26, 0x72), // 1 red      #f92672
                RgbColor::new(0xa6, 0xe2, 0x2e), // 2 green    #a6e22e
                RgbColor::new(0xe6, 0xdb, 0x74), // 3 yellow   #e6db74
                RgbColor::new(0xfd, 0x97, 0x1f), // 4 blue/org #fd971f
                RgbColor::new(0xae, 0x81, 0xff), // 5 magenta  #ae81ff
                RgbColor::new(0x66, 0xd9, 0xef), // 6 cyan     #66d9ef
                RgbColor::new(0xfd, 0xff, 0xf1), // 7 white    #fdfff1
                // Bright colors (8-15)
                RgbColor::new(0x6e, 0x70, 0x66), // 8  bright black   #6e7066
                RgbColor::new(0xf9, 0x26, 0x72), // 9  bright red     #f92672
                RgbColor::new(0xa6, 0xe2, 0x2e), // 10 bright green   #a6e22e
                RgbColor::new(0xe6, 0xdb, 0x74), // 11 bright yellow  #e6db74
                RgbColor::new(0xfd, 0x97, 0x1f), // 12 bright blue    #fd971f
                RgbColor::new(0xae, 0x81, 0xff), // 13 bright magenta #ae81ff
                RgbColor::new(0x66, 0xd9, 0xef), // 14 bright cyan    #66d9ef
                RgbColor::new(0xfd, 0xff, 0xf1), // 15 bright white   #fdfff1
            ],
        }
    }
}
