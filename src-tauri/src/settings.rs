//! User settings persisted as JSON in the app data dir.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

#[cfg(target_os = "macos")]
pub const DEFAULT_SHORTCUT: &str = "Alt+Super+V";
#[cfg(not(target_os = "macos"))]
pub const DEFAULT_SHORTCUT: &str = "Super+Alt+V";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub shortcut: String,
    pub retention: Retention,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_settings_keep_shortcut_and_receive_retention_defaults() {
        let settings: Settings = serde_json::from_str(r#"{"shortcut":"Ctrl+Alt+X"}"#).unwrap();
        assert_eq!(settings.shortcut, "Ctrl+Alt+X");
        assert!(settings.retention.days_enabled && settings.retention.size_enabled);
        assert_eq!(settings.retention.days, 30);
        assert_eq!(settings.retention.max_mb, 1024);
    }
    #[test]
    fn unchecked_conditions_and_values_survive_serialization() {
        let mut settings = Settings::default();
        settings.retention.days_enabled = false;
        settings.retention.size_enabled = false;
        settings.retention.days = 90;
        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert!(!restored.retention.days_enabled && !restored.retention.size_enabled);
        assert_eq!(restored.retention.days, 90);
        assert!(Retention {
            max_mb: 0,
            ..Retention::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn saved_policy_survives_restart_and_failed_write_preserves_previous_policy() {
        let directory = std::env::temp_dir().join(format!(
            "pinchy-settings-{}-{}",
            std::process::id(),
            crate::db::now_ms()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("settings.json");
        let store = SettingsStore::load(path.clone());
        store.update(|s| s.shortcut = "Ctrl+Alt+X".into()).unwrap();
        store
            .update(|s| {
                s.retention.days_enabled = false;
                s.retention.size_enabled = false;
            })
            .unwrap();
        let restored = SettingsStore::load(path.clone());
        assert_eq!(restored.get().shortcut, "Ctrl+Alt+X");
        assert!(!restored.get().retention.days_enabled);
        assert!(!restored.get().retention.size_enabled);
        // Block the temporary write without relying on platform permissions.
        std::fs::create_dir(path.with_extension("json.tmp")).unwrap();
        assert!(store.update(|s| s.retention.days_enabled = true).is_err());
        assert!(!store.get().retention.days_enabled);
        assert!(!SettingsStore::load(path).get().retention.days_enabled);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Retention {
    pub days_enabled: bool,
    pub days: u32,
    pub size_enabled: bool,
    pub max_mb: u32,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            days_enabled: true,
            days: 30,
            size_enabled: true,
            max_mb: 1024,
        }
    }
}

impl Retention {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=36500).contains(&self.days) {
            return Err("보관 기간은 1~36,500일 사이의 정수로 입력해 주세요.".into());
        }
        if !(1..=1_048_576).contains(&self.max_mb) {
            return Err("최대 용량은 1~1,048,576MB 사이의 정수로 입력해 주세요.".into());
        }
        Ok(())
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            shortcut: DEFAULT_SHORTCUT.into(),
            retention: Retention::default(),
        }
    }
}

pub struct SettingsStore {
    path: PathBuf,
    pub current: Mutex<Settings>,
}

impl SettingsStore {
    pub fn load(path: PathBuf) -> Self {
        let current = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            path,
            current: Mutex::new(current),
        }
    }

    pub fn get(&self) -> Settings {
        self.current.lock().unwrap().clone()
    }

    pub fn update(&self, update: impl FnOnce(&mut Settings)) -> Result<(), String> {
        let mut current = self.current.lock().unwrap();
        let mut settings = current.clone();
        update(&mut settings);
        let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
        // A failed write must not truncate the old policy and silently restore
        // default deletion rules on the next launch.
        let temporary = self.path.with_extension("json.tmp");
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&temporary).map_err(|e| e.to_string())?;
            file.write_all(json.as_bytes()).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
        }
        std::fs::rename(&temporary, &self.path).map_err(|e| e.to_string())?;
        *current = settings;
        Ok(())
    }
}
