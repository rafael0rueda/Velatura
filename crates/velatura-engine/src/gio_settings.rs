//! Desktop settings through GIO: the same store GNOME Settings writes to.

use gio::prelude::*;

use crate::host::{SettingState, Settings, SettingsError};

/// The settings of the running session.
pub struct GioSettings {
    backend: String,
}

impl GioSettings {
    pub fn new() -> Self {
        Self {
            backend: gio::SettingsBackend::default().type_().name().to_owned(),
        }
    }

    /// The name of the store in use. Anything but dconf means changes would
    /// not reach the desktop, or would not survive the process.
    pub fn backend(&self) -> &str {
        &self.backend
    }

    fn usable(&self) -> Result<(), SettingsError> {
        if self.backend == "DConfSettingsBackend" {
            Ok(())
        } else {
            Err(SettingsError::Unavailable(self.backend.clone()))
        }
    }

    /// `None` if the schema or key is not installed. Looked up first because
    /// GIO aborts the process when asked for a schema that does not exist.
    fn open(schema: &str, key: &str) -> Option<gio::Settings> {
        let found = gio::SettingsSchemaSource::default()?.lookup(schema, true)?;
        found.has_key(key).then(|| gio::Settings::new(schema))
    }
}

impl Default for GioSettings {
    fn default() -> Self {
        Self::new()
    }
}

impl Settings for GioSettings {
    fn read(&self, schema: &str, key: &str) -> Result<Option<SettingState>, SettingsError> {
        self.usable()?;
        let Some(settings) = Self::open(schema, key) else {
            return Ok(None);
        };
        let text = |v: gio::glib::Variant| match v.str() {
            Some(s) => s.to_owned(),
            None => v.print(false).to_string(),
        };
        let effective = text(settings.value(key));
        Ok(Some(SettingState {
            user: settings.user_value(key).map(text),
            effective,
            writable: settings.is_writable(key),
        }))
    }

    fn write(&self, schema: &str, key: &str, value: Option<&str>) -> Result<(), SettingsError> {
        self.usable()?;
        let Some(settings) = Self::open(schema, key) else {
            return Err(SettingsError::Failed("no such setting".to_owned()));
        };
        if !settings.is_writable(key) {
            return Err(SettingsError::NotWritable);
        }
        match value {
            Some(v) => settings
                .set_string(key, v)
                .map_err(|e| SettingsError::Failed(e.to_string())),
            None => {
                settings.reset(key);
                Ok(())
            }
        }
    }

    fn sync(&self) {
        gio::Settings::sync();
    }
}
