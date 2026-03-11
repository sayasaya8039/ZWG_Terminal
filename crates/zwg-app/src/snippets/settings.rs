use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const SNIPPET_SETTINGS_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SnippetSettings {
    pub version: u32,
    pub general: GeneralSettings,
    pub hotkeys: HotkeySettings,
    pub display: DisplaySettings,
    pub advanced: AdvancedSettings,
    pub filters: FilterSettings,
    pub notifications: NotificationSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    pub max_history_items: usize,
    pub launch_on_startup: bool,
    pub show_in_taskbar: bool,
    pub play_sound: bool,
    pub check_updates: bool,
    pub minimize_to_tray: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeySettings {
    pub show_window: String,
    pub paste_as_plain_text: String,
    pub quick_paste: String,
    pub show_favorites: String,
    pub show_templates: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaySettings {
    pub theme: String,
    pub font_size: u8,
    pub transparency: u8,
    pub show_preview: bool,
    pub window_position: String,
    pub items_per_page: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvancedSettings {
    pub auto_save: bool,
    pub exclude_apps: String,
    pub fifo_mode: bool,
    pub max_clipboard_size: usize,
    pub enable_password: bool,
    pub monitor_clipboard: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterSettings {
    pub ignore_case: bool,
    pub trim_whitespace: bool,
    pub ignore_duplicates: bool,
    pub min_text_length: usize,
    pub exclude_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationSettings {
    pub show_copy_notification: bool,
    pub show_paste_notification: bool,
    pub notification_position: String,
    pub notification_duration: u64,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            max_history_items: 1000,
            launch_on_startup: false,
            show_in_taskbar: true,
            play_sound: false,
            check_updates: true,
            minimize_to_tray: false,
        }
    }
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            show_window: "Ctrl+Shift+V".to_string(),
            paste_as_plain_text: "Ctrl+Shift+Enter".to_string(),
            quick_paste: "Ctrl+Shift+1".to_string(),
            show_favorites: "Ctrl+Shift+F".to_string(),
            show_templates: "Ctrl+Shift+T".to_string(),
        }
    }
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            theme: "light".to_string(),
            font_size: 14,
            transparency: 100,
            show_preview: true,
            window_position: "center".to_string(),
            items_per_page: 20,
        }
    }
}

impl Default for AdvancedSettings {
    fn default() -> Self {
        Self {
            auto_save: true,
            exclude_apps: String::new(),
            fifo_mode: false,
            max_clipboard_size: 10_000,
            enable_password: false,
            monitor_clipboard: false,
        }
    }
}

impl Default for FilterSettings {
    fn default() -> Self {
        Self {
            ignore_case: true,
            trim_whitespace: true,
            ignore_duplicates: false,
            min_text_length: 1,
            exclude_patterns: Vec::new(),
        }
    }
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            show_copy_notification: false,
            show_paste_notification: true,
            notification_position: "topRight".to_string(),
            notification_duration: 2000,
        }
    }
}

impl Default for SnippetSettings {
    fn default() -> Self {
        Self {
            version: SNIPPET_SETTINGS_VERSION,
            general: GeneralSettings::default(),
            hotkeys: HotkeySettings::default(),
            display: DisplaySettings::default(),
            advanced: AdvancedSettings::default(),
            filters: FilterSettings::default(),
            notifications: NotificationSettings::default(),
        }
    }
}

impl SnippetSettings {
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("zwg")
            .join("snippet-settings.json")
    }

    pub fn load() -> Self {
        Self::load_from_path(Self::default_path())
    }

    pub fn load_from_path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str::<Self>(&content)
                .map(Self::validated)
                .unwrap_or_else(|error| {
                    log::warn!("Invalid snippet settings file at {:?}: {}", path, error);
                    Self::default()
                }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let settings = Self::default();
                let _ = settings.save();
                settings
            }
            Err(error) => {
                log::warn!("Failed to read snippet settings at {:?}: {}", path, error);
                Self::default()
            }
        }
    }

    pub fn save(&self) -> io::Result<()> {
        self.save_to_path(Self::default_path())
    }

    pub fn save_to_path(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json =
            serde_json::to_string_pretty(&self.clone().validated()).map_err(io::Error::other)?;
        fs::write(path, json)?;
        Ok(())
    }

    pub fn import_from_path(path: impl AsRef<Path>) -> io::Result<Self> {
        let content = fs::read_to_string(path)?;
        serde_json::from_str::<Self>(&content)
            .map(Self::validated)
            .map_err(io::Error::other)
    }

    pub fn validated(mut self) -> Self {
        self.version = SNIPPET_SETTINGS_VERSION;
        self.general.max_history_items = self.general.max_history_items.clamp(100, 5000);
        self.display.font_size = self.display.font_size.clamp(10, 20);
        self.display.transparency = self.display.transparency.clamp(50, 100);
        self.display.items_per_page = self.display.items_per_page.clamp(10, 100);
        self.advanced.max_clipboard_size = self.advanced.max_clipboard_size.clamp(1000, 50_000);
        self.filters.min_text_length = self.filters.min_text_length.clamp(1, 1000);
        self.notifications.notification_duration =
            self.notifications.notification_duration.clamp(1000, 5000);
        self.filters.exclude_patterns = self
            .filters
            .exclude_patterns
            .iter()
            .map(|pattern| pattern.trim())
            .filter(|pattern| !pattern.is_empty())
            .map(str::to_string)
            .collect();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_preserves_values() {
        let settings = SnippetSettings::default();
        let json = serde_json::to_string(&settings).unwrap();
        let restored: SnippetSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.version, SNIPPET_SETTINGS_VERSION);
        assert_eq!(restored.display.theme, "light");
        assert_eq!(restored.notifications.notification_duration, 2000);
    }
}
