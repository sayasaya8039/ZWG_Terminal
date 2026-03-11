//! Configuration module — settings, themes, and persistence

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[cfg(windows)]
use std::process::Command;

pub const CONFIG_VERSION: u32 = 2;
const DEFAULT_WINDOW_COLS: u16 = 120;
const DEFAULT_WINDOW_ROWS: u16 = 30;
#[cfg(windows)]
const RUN_KEY_PATH: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const RUN_VALUE_NAME: &str = "ZWG Terminal";

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
#[serde(default)]
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
#[serde(default)]
pub struct AppConfig {
    pub version: u32,
    pub shell: String,
    pub font: FontConfig,
    pub theme: String, // theme name
    pub scrollback_lines: usize,
    pub cursor_blink: bool,
    pub tab_bar_visible: bool,
    pub status_bar_visible: bool,
    pub launch_on_login: bool,
    pub confirm_on_close: bool,
    pub default_window_cols: u16,
    pub default_window_rows: u16,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            shell: crate::shell::detect_default_shell(),
            font: FontConfig::default(),
            theme: "Catppuccin Mocha".into(),
            scrollback_lines: 10_000,
            cursor_blink: true,
            tab_bar_visible: true,
            status_bar_visible: true,
            launch_on_login: false,
            confirm_on_close: true,
            default_window_cols: DEFAULT_WINDOW_COLS,
            default_window_rows: DEFAULT_WINDOW_ROWS,
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
                Ok(content) => match serde_json::from_str::<AppConfig>(&content) {
                    Ok(config) => {
                        log::info!("Loaded config from {:?}", path);
                        return config.validated().sync_launch_on_login();
                    }
                    Err(e) => log::warn!("Invalid config file: {}", e),
                },
                Err(e) => log::warn!("Failed to read config: {}", e),
            }
        }
        Self::default().sync_launch_on_login()
    }

    /// Clamp config values to safe ranges
    fn validated(mut self) -> Self {
        self.version = CONFIG_VERSION;
        if self.shell.trim().is_empty() {
            log::warn!("Empty shell in config, using default");
            self.shell = crate::shell::detect_default_shell();
        }
        self.scrollback_lines = self.scrollback_lines.clamp(100, 100_000);
        self.font.size = self.font.size.clamp(6.0, 72.0);
        self.font.line_height = self.font.line_height.clamp(1.0, 3.0);
        self.default_window_cols = self.default_window_cols.clamp(60, 240);
        self.default_window_rows = self.default_window_rows.clamp(18, 120);
        self
    }

    pub fn sanitized(self) -> Self {
        self.validated()
    }

    fn sync_launch_on_login(mut self) -> Self {
        if let Ok(enabled) = launch_on_login_enabled() {
            self.launch_on_login = enabled;
        }
        self
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

pub fn launch_on_login_enabled() -> std::io::Result<bool> {
    #[cfg(windows)]
    {
        let output = Command::new("reg")
            .args(["query", RUN_KEY_PATH, "/v", RUN_VALUE_NAME])
            .output()?;
        return Ok(output.status.success());
    }

    #[cfg(not(windows))]
    {
        Ok(false)
    }
}

pub fn set_launch_on_login(enabled: bool) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let output = if enabled {
            let exe = std::env::current_exe()?;
            let command = format!("\"{}\"", exe.display());
            Command::new("reg")
                .args([
                    "add",
                    RUN_KEY_PATH,
                    "/v",
                    RUN_VALUE_NAME,
                    "/t",
                    "REG_SZ",
                    "/d",
                    &command,
                    "/f",
                ])
                .output()?
        } else {
            Command::new("reg")
                .args(["delete", RUN_KEY_PATH, "/v", RUN_VALUE_NAME, "/f"])
                .output()?
        };

        if output.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(std::io::Error::other(stderr.trim().to_string()));
    }

    #[cfg(not(windows))]
    {
        let _ = enabled;
        Ok(())
    }
}

/// Window position and size state (persisted separately from config)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            x: 100.0,
            y: 100.0,
            width: 1400.0,
            height: 900.0,
            maximized: false,
        }
    }
}

impl WindowState {
    /// State file path: ~/.config/zwg/window_state.json
    fn state_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("zwg")
            .join("window_state.json")
    }

    /// Load window state from disk, falling back to defaults
    pub fn load() -> Self {
        let path = Self::state_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<WindowState>(&content) {
                    Ok(state) => {
                        log::info!("Loaded window state from {:?}", path);
                        return state.validated();
                    }
                    Err(e) => log::warn!("Invalid window state file: {}", e),
                },
                Err(e) => log::warn!("Failed to read window state: {}", e),
            }
        }
        Self::default()
    }

    /// Save window state to disk
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::state_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&path, json)?;
        log::debug!("Saved window state to {:?}", path);
        Ok(())
    }

    /// Clamp to reasonable bounds
    fn validated(mut self) -> Self {
        self.width = self.width.clamp(400.0, 7680.0);
        self.height = self.height.clamp(300.0, 4320.0);
        // Allow negative coords for multi-monitor setups, but clamp extremes
        self.x = self.x.clamp(-4000.0, 7680.0);
        self.y = self.y.clamp(-4000.0, 4320.0);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_config() -> AppConfig {
        AppConfig {
            version: CONFIG_VERSION,
            shell: "cmd.exe".into(),
            font: FontConfig::default(),
            theme: "Catppuccin Mocha".into(),
            scrollback_lines: 10_000,
            cursor_blink: true,
            tab_bar_visible: true,
            status_bar_visible: true,
            launch_on_login: false,
            confirm_on_close: true,
            default_window_cols: DEFAULT_WINDOW_COLS,
            default_window_rows: DEFAULT_WINDOW_ROWS,
        }
    }

    #[test]
    fn config_round_trip_serde() {
        let config = AppConfig {
            version: CONFIG_VERSION,
            shell: "cmd.exe".into(),
            font: FontConfig {
                family: "Consolas".into(),
                size: 16.0,
                line_height: 1.5,
            },
            theme: "Tokyo Night".into(),
            scrollback_lines: 5000,
            cursor_blink: false,
            tab_bar_visible: true,
            status_bar_visible: false,
            launch_on_login: true,
            confirm_on_close: false,
            default_window_cols: 132,
            default_window_rows: 40,
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.version, CONFIG_VERSION);
        assert_eq!(restored.shell, "cmd.exe");
        assert_eq!(restored.font.family, "Consolas");
        assert_eq!(restored.font.size, 16.0);
        assert_eq!(restored.theme, "Tokyo Night");
        assert_eq!(restored.scrollback_lines, 5000);
        assert!(!restored.cursor_blink);
        assert!(!restored.status_bar_visible);
        assert!(restored.launch_on_login);
        assert!(!restored.confirm_on_close);
        assert_eq!(restored.default_window_cols, 132);
        assert_eq!(restored.default_window_rows, 40);
    }

    #[test]
    fn config_validated_clamps_scrollback() {
        let config = AppConfig {
            scrollback_lines: 0,
            ..make_test_config()
        };
        let v = config.validated();
        assert_eq!(v.scrollback_lines, 100);

        let config2 = AppConfig {
            scrollback_lines: 999_999,
            ..make_test_config()
        };
        let v2 = config2.validated();
        assert_eq!(v2.scrollback_lines, 100_000);
    }

    #[test]
    fn config_validated_clamps_font_size() {
        let mut config = make_test_config();
        config.font.size = 1.0;
        assert_eq!(config.validated().font.size, 6.0);

        let mut config2 = make_test_config();
        config2.font.size = 200.0;
        assert_eq!(config2.validated().font.size, 72.0);
    }

    #[test]
    fn config_validated_clamps_line_height() {
        let mut config = make_test_config();
        config.font.line_height = 0.5;
        assert_eq!(config.validated().font.line_height, 1.0);

        let mut config2 = make_test_config();
        config2.font.line_height = 5.0;
        assert_eq!(config2.validated().font.line_height, 3.0);
    }

    #[test]
    fn config_validated_clamps_window_grid() {
        let mut config = make_test_config();
        config.default_window_cols = 10;
        config.default_window_rows = 5;
        let validated = config.validated();
        assert_eq!(validated.default_window_cols, 60);
        assert_eq!(validated.default_window_rows, 18);

        let mut config2 = make_test_config();
        config2.default_window_cols = 999;
        config2.default_window_rows = 999;
        let validated2 = config2.validated();
        assert_eq!(validated2.default_window_cols, 240);
        assert_eq!(validated2.default_window_rows, 120);
    }

    #[test]
    fn config_validated_empty_shell_uses_default() {
        let mut config = make_test_config();
        config.shell = "   ".into();
        let v = config.validated();
        assert!(!v.shell.trim().is_empty());
    }

    #[test]
    fn config_malformed_json_returns_err() {
        let bad = r#"{"shell": "cmd.exe", "font": {"broken"#;
        let result: Result<AppConfig, _> = serde_json::from_str(bad);
        assert!(result.is_err());
    }

    #[test]
    fn theme_mocha_colors() {
        let t = Theme::mocha();
        assert_eq!(t.name, "Catppuccin Mocha");
        assert_eq!(t.base, 0x1e1e2e);
        assert_eq!(t.fg, 0xcdd6f4);
    }

    #[test]
    fn theme_latte_is_light() {
        let t = Theme::latte();
        assert_eq!(t.name, "Catppuccin Latte");
        assert!(t.bg > 0x800000);
    }

    #[test]
    fn active_theme_selects_correctly() {
        let config = AppConfig {
            theme: "Tokyo Night".into(),
            ..make_test_config()
        };
        assert_eq!(config.active_theme().name, "Tokyo Night");
    }

    #[test]
    fn active_theme_unknown_falls_back_to_mocha() {
        let config = AppConfig {
            theme: "Nonexistent".into(),
            ..make_test_config()
        };
        assert_eq!(config.active_theme().name, "Catppuccin Mocha");
    }

    #[test]
    fn available_themes_has_three() {
        let themes = AppConfig::available_themes();
        assert_eq!(themes.len(), 3);
        assert!(themes.contains(&"Catppuccin Mocha"));
        assert!(themes.contains(&"Tokyo Night"));
    }

    #[test]
    fn font_config_default_values() {
        let f = FontConfig::default();
        assert_eq!(f.family, "Cascadia Code");
        assert_eq!(f.size, 14.0);
        assert_eq!(f.line_height, 1.3);
    }
}
