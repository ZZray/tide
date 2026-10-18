//! Explicitly portable preferences. Never serialize the machine configuration.
use crate::{Config, Notification, Theme};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub version: u32,
    pub theme: Theme,
    pub notify: Notification,
}
impl Preferences {
    pub fn from_config(config: &Config) -> Self {
        Self {
            version: 1,
            theme: config.app.theme,
            notify: config.app.notify,
        }
    }
    pub fn load(path: &Path) -> Result<Self> {
        let mut text = String::new();
        File::open(path)?.take(65537).read_to_string(&mut text)?;
        ensure!(text.len() <= 65536, "preferences file exceeds 64 KiB");
        let preferences: Self = toml::from_str(&text)?;
        ensure!(preferences.version == 1, "unsupported preferences version");
        Ok(preferences)
    }
    pub fn export(&self, destination: &Path) -> Result<()> {
        ensure!(self.version == 1, "unsupported preferences version");
        let parent = destination
            .parent()
            .context("preferences path has no parent")?;
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(toml::to_string_pretty(self)?.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist_noclobber(destination).map_err(|e| e.error)?;
        Ok(())
    }
    pub fn apply(&self, path: &Path) -> Result<()> {
        ensure!(self.version == 1, "unsupported preferences version");
        Config::edit(path, |config| {
            config.app.theme = self.theme;
            config.app.notify = self.notify;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_preferences_preserve_local_settings_and_refuse_full_config() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.toml");
        Config::init(&config_path).unwrap();
        Config::edit(&config_path, |config| {
            config.app.device_name = "Local device".into();
            config.app.autostart = true;
            config.app.close_to_tray = false;
            Ok(())
        })
        .unwrap();
        let preferences = Preferences {
            version: 1,
            theme: Theme::Light,
            notify: Notification::Off,
        };
        let portable = temp.path().join("synced/.tide/preferences.toml");
        preferences.export(&portable).unwrap();
        assert!(preferences.export(&portable).is_err());
        Preferences::load(&portable)
            .unwrap()
            .apply(&config_path)
            .unwrap();
        let actual = Config::load(&config_path).unwrap();
        assert_eq!(actual.app.theme, Theme::Light);
        assert_eq!(actual.app.notify, Notification::Off);
        assert_eq!(actual.app.device_name, "Local device");
        assert!(actual.app.autostart);
        assert!(!actual.app.close_to_tray);
        assert!(Preferences::load(&config_path).is_err());
        std::fs::write(&portable, "version = 2\ntheme = 'light'\nnotify = 'off'\n").unwrap();
        assert!(Preferences::load(&portable).is_err());
        std::fs::write(
            &portable,
            "version = 1\ntheme = 'light'\nnotify = 'off'\nautostart = true\n",
        )
        .unwrap();
        assert!(Preferences::load(&portable).is_err());
    }
}
