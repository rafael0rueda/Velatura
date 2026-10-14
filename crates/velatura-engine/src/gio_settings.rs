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
    /// installed for one user keeps its own. Or `id:/path/`, for a schema
    /// that has no place of its own in the store and is given one, as each
    /// profile of a terminal is; the same form `gsettings` takes.
    fn open(schema: &str, key: &str) -> Option<gio::Settings> {
        let default = gio::SettingsSchemaSource::default()?;
        if let Some((id, path)) = schema.split_once(':') {
            let found = default.lookup(id, true)?;
            // GIO aborts on a path that is not one, and on a schema that
            // has a place of its own being given another.
            let well_formed = path.starts_with('/') && path.ends_with('/') && !path.contains("//");
            return (well_formed && found.path().is_none() && found.has_key(key)).then(|| {
                gio::Settings::new_full(&found, None::<&gio::SettingsBackend>, Some(path))
            });
        }
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

/// A text property of a program on the session bus, if that program is
/// running. It is never started for the question.
fn bus_property(name: &str, key: &str) -> Option<String> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok()?;
    let path = format!("/{}", name.replace('.', "/"));
    let reply = bus
        .call_sync(
            Some(name),
            &path,
            "org.freedesktop.DBus.Properties",
            "Get",
            Some(&(name, key).to_variant()),
            None,
            gio::DBusCallFlags::NO_AUTO_START,
            500,
            gio::Cancellable::NONE,
        )
        .ok()?;
    let value = reply.child_value(0).as_variant()?;
    value.str().map(str::to_owned)
}

impl Settings for GioSettings {
    fn read(&self, schema: &str, key: &str) -> Result<Option<SettingState>, SettingsError> {
        self.usable()?;
        if let Some(name) = schema.strip_prefix("dbus:") {
            return Ok(bus_property(name, key).map(|effective| SettingState {
                user: None,
                effective,
                writable: false,
            }));
        }
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
        if schema.starts_with("dbus:") {
            return Err(SettingsError::NotWritable);
        }
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

/// The local time of day at `now` (seconds since the Unix epoch), in minutes
/// after midnight. `None` if the time zone cannot be worked out.
pub fn local_minute(now: u64) -> Option<u16> {
    let local = gio::glib::DateTime::from_unix_local(i64::try_from(now).ok()?).ok()?;
    u16::try_from(local.hour() * 60 + local.minute()).ok()
}
