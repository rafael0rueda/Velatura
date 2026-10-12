//! Desktop settings through GIO: the same store GNOME Settings writes to.

use gio::prelude::*;

use crate::host::{SettingState, Settings, SettingsError};

/// The settings of the running session.
pub struct GioSettings {
    backend: String,
    /// Whether there is a session bus. Without one the store can be read
    /// but a write is silently lost, so it is treated as unavailable.
    bus: bool,
}

impl GioSettings {
    pub fn new() -> Self {
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
        let bus = var("DBUS_SESSION_BUS_ADDRESS").is_some()
            || var("XDG_RUNTIME_DIR")
                .is_some_and(|dir| std::path::Path::new(&dir).join("bus").exists());
        Self {
            backend: gio::SettingsBackend::default().type_().name().to_owned(),
            bus,
        }
    }

    /// The name of the store in use. Anything but dconf means changes would
    /// not reach the desktop, or would not survive the process.
    pub fn backend(&self) -> &str {
        &self.backend
    }

    fn usable(&self) -> Result<(), SettingsError> {
        if !self.bus {
            Err(SettingsError::Unavailable("no desktop session".to_owned()))
        } else if self.backend == "DConfSettingsBackend" {
            Ok(())
        } else {
            Err(SettingsError::Unavailable(self.backend.clone()))
        }
    }

    /// `None` if the schema or key is not installed. Looked up first because
    /// GIO aborts the process when asked for a schema that does not exist.
    ///
    /// `schema` may be `id@/folder`: the schema is then taken from the
    /// compiled schemas in that folder, which is where an extension
    /// installed for one user keeps its own.
    fn open(schema: &str, key: &str) -> Option<gio::Settings> {
        let default = gio::SettingsSchemaSource::default()?;
        let found = match schema.split_once('@') {
            Some((id, dir)) => {
                gio::SettingsSchemaSource::from_directory(dir, Some(&default), false)
                    .ok()?
                    .lookup(id, false)?
            }
            None => default.lookup(schema, true)?,
        };
        found
            .has_key(key)
            .then(|| gio::Settings::new_full(&found, None::<&gio::SettingsBackend>, None))
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
