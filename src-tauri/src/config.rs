use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    #[serde(default = "default_theme")]
    pub theme: String,
}

fn default_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}

fn default_model() -> String {
    "gpt-4o-mini".to_string()
}

fn default_hotkey() -> String {
    "Cmd+.".to_string()
}

fn default_theme() -> String {
    "system".to_string()
}

impl Config {
    /// Get the config file path: ~/.config/compleo/config.json
    fn file_path() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".config").join("compleo").join("config.json")
    }

    /// Load config from file, falling back to environment variables
    pub fn load() -> Self {
        let path = Self::file_path();

        let mut config = if path.exists() {
            match fs::read_to_string(&path) {
                Ok(content) => serde_json::from_str::<Config>(&content).unwrap_or_default(),
                Err(_) => Config::default(),
            }
        } else {
            Config::default()
        };

        // Override with environment variables if config values are empty
        if config.api_key.is_empty() {
            if let Ok(key) = std::env::var("OPENAI_API_KEY").or_else(|_| std::env::var("COMPLEO_API_KEY")) {
                config.api_key = key;
            }
        }
        if config.base_url == default_base_url() || config.base_url.is_empty() {
            if let Ok(url) = std::env::var("COMPLEO_BASE_URL") {
                config.base_url = url;
            }
        }
        if config.model == default_model() || config.model.is_empty() {
            if let Ok(model) = std::env::var("COMPLEO_MODEL") {
                config.model = model;
            }
        }

        // Ensure defaults for empty values
        if config.base_url.is_empty() {
            config.base_url = default_base_url();
        }
        if config.model.is_empty() {
            config.model = default_model();
        }
        if config.hotkey.is_empty() {
            config.hotkey = default_hotkey();
        }

        config
    }

    /// Save config to file
    pub fn save(&self) -> Result<(), String> {
        let path = Self::file_path();

        // Create directory if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Failed to create config dir: {}", e))?;
        }

        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        fs::write(&path, json).map_err(|e| format!("Failed to write config: {}", e))?;

        log::info!("Config saved to {:?}", path);
        Ok(())
    }
}

// Tauri IPC commands

#[tauri::command]
pub fn get_config() -> Result<Config, String> {
    Ok(Config::load())
}

#[tauri::command]
pub fn save_config(config: Config) -> Result<(), String> {
    config.save()
}
