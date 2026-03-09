//! Configuration module — settings, themes, and persistence

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Color theme definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub base: u32,
    pub mantle: u32,
    pub crust: u32,
    pub surface0: u32,
    pub surface1: u32,
    pub surface2: u32,
    pub text: u32,
    pub subtext0: u32,
    pub subtext1: u32,
    pub red: u32,
    pub green: u32,
    pub blue: u32,
    pub yellow: u32,
    pub mauve: u32,
    pub fg: u32,
    pub bg: u32,
}

impl Theme {
    /// Catppuccin Mocha (default dark theme)
    pub fn mocha() -> Self {
        Self {
            name: "Catppuccin Mocha".into(),
            base: 0x1e1e2e,
            mantle: 0x181825,
            crust: 0x11111b,
            surface0: 0x313244,
            surface1: 0x45475a,
            surface2: 0x585b70,
            text: 0xcdd6f4,
            subtext0: 0xa6adc8,
            subtext1: 0xbac2de,
            red: 0xf38ba8,
            green: 0xa6e3a1,
            blue: 0x89b4fa,
            yellow: 0xf9e2af,
            mauve: 0xcba6f7,
            fg: 0xcdd6f4,
            bg: 0x1e1e2e,
        }
    }

    /// Catppuccin Latte (light theme)
    pub fn latte() -> Self {
        Self {
            name: "Catppuccin Latte".into(),
            base: 0xeff1f5,
            mantle: 0xe6e9ef,
            crust: 0xdce0e8,
            surface0: 0xccd0da,
            surface1: 0xbcc0cc,
            surface2: 0xacb0be,
            text: 0x4c4f69,
            subtext0: 0x6c6f85,
            subtext1: 0x5c5f77,
            red: 0xd20f39,
            green: 0x40a02b,
            blue: 0x1e66f5,
            yellow: 0xdf8e1d,
            mauve: 0x8839ef,
            fg: 0x4c4f69,
            bg: 0xeff1f5,
        }
    }

    /// Tokyo Night
    pub fn tokyo_night() -> Self {
        Self {
            name: "Tokyo Night".into(),
            base: 0x1a1b26,
            mantle: 0x16161e,
            crust: 0x13131a,
            surface0: 0x292e42,
            surface1: 0x3b4261,
            surface2: 0x545c7e,
            text: 0xc0caf5,
            subtext0: 0xa9b1d6,
            subtext1: 0x9aa5ce,
            red: 0xf7768e,
            green: 0x9ece6a,
            blue: 0x7aa2f7,
            yellow: 0xe0af68,
            mauve: 0xbb9af7,
            fg: 0xc0caf5,
            bg: 0x1a1b26,
        }
    }
}

/// Font configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
    pub line_height: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "Cascadia Code".into(),
            size: 14.0,
            line_height: 1.3,
        }
    }
}

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub shell: String,
    pub font: FontConfig,
    pub theme: String, // theme name
    pub scrollback_lines: usize,
    pub cursor_blink: bool,
    pub tab_bar_visible: bool,
    pub status_bar_visible: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            shell: crate::shell::detect_default_shell(),
            font: FontConfig::default(),
            theme: "Catppuccin Mocha".into(),
            scrollback_lines: 10_000,
            cursor_blink: true,
            tab_bar_visible: true,
            status_bar_visible: true,
        }
    }
}

impl AppConfig {
    /// Config file path: ~/.config/zwg/config.json
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("zwg")
            .join("config.json")
    }

    /// Load config from disk, falling back to defaults
    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str(&content) {
                    Ok(config) => {
                        log::info!("Loaded config from {:?}", path);
                        return config;
                    }
                    Err(e) => log::warn!("Invalid config file: {}", e),
                },
                Err(e) => log::warn!("Failed to read config: {}", e),
            }
        }
        Self::default()
    }

    /// Save config to disk
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&path, json)?;
        log::info!("Saved config to {:?}", path);
        Ok(())
    }

    /// Get the active theme
    pub fn active_theme(&self) -> Theme {
        match self.theme.as_str() {
            "Catppuccin Latte" => Theme::latte(),
            "Tokyo Night" => Theme::tokyo_night(),
            _ => Theme::mocha(),
        }
    }

    /// Available theme names
    pub fn available_themes() -> Vec<&'static str> {
        vec!["Catppuccin Mocha", "Catppuccin Latte", "Tokyo Night"]
    }
}
