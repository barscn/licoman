//! GSettings read backend (05_READ_ONLY_BACKENDS.md §2.A).
//!
//! Reads **effective** values the way the platform stores them, without
//! spawning processes and without writing anything:
//!
//! - **Schema defaults** come from the compiled schema databases
//!   (`gschemas.compiled`, a GVDB file) searched in XDG data-dir order —
//!   the same precedence GLib uses. Schema/key availability is probed
//!   before any read; a missing schema or key is a normal reportable state.
//! - **User and managed overrides** come from dconf databases (also GVDB):
//!   the user database first (highest priority), then the system databases
//!   listed in the dconf profile, per dconf's documented rule ("The user's
//!   personal database always takes the highest priority, followed by the
//!   system databases in the order prescribed by the profile").
//! - Origin mapping: override in a system db → [`ValueOrigin::Managed`],
//!   user db → [`ValueOrigin::User`], otherwise the schema default
//!   → [`ValueOrigin::Default`]. A missing key is an error, never an empty
//!   value.
//!
//! All file reads are bounded; every byte-level failure degrades to an
//! error with a user-safe message. Change detection: the dconf user
//! database path is exposed for the debounced watcher — dconf-service
//! rewrites that file on every `gsettings set`, so edits made by other
//! tools become visible (05_READ_ONLY_BACKENDS.md §3).

use std::path::{Path, PathBuf};

use crate::descriptor::SettingDescriptor;
use crate::discovery::gsettings as gvdb;
use crate::error::{ErrorKind, LicomanError};
use crate::modules::def::BackendBinding;
use crate::read::gvariant::{GVariantValue, decode_framed};
use crate::read::{
    Freshness, ReadResult, RestartEffect, SettingsReadBackend, SourceDetails, SupportState,
    ValueOrigin, Writable, fnv1a_fingerprint, read_bounded,
};
use crate::value::SettingValue;

/// Upper bound for one GVDB database file (the system schema db is ~1.5 MiB
/// on a full desktop; anything beyond this is not a schema or dconf db we
/// should parse).
const MAX_DB_BYTES: usize = 64 * 1024 * 1024;

/// Injectable search roots so tests and demo fixtures never touch the real
/// environment.
#[derive(Debug, Clone)]
pub struct GSettingsReadBackend {
    /// Directories searched for `glib-2.0/schemas/gschemas.compiled`, in
    /// GLib's priority order (first file containing the schema wins).
    schema_dirs: Vec<PathBuf>,
    /// The dconf user database file (highest priority).
    dconf_user_db: Option<PathBuf>,
    /// Additional dconf databases in profile priority order (system dbs).
    dconf_system_dbs: Vec<PathBuf>,
    /// Environment notes surfaced as degraded-state diagnostics.
    notes: Vec<String>,
}

impl GSettingsReadBackend {
    /// Build the backend from the process environment (the production
    /// constructor). Never fails: missing pieces become honest
    /// unavailability reasons at read time.
    pub fn from_env() -> Self {
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());

        // Schema sources: GSETTINGS_SCHEMA_DIR (developer override), then
        // XDG_DATA_HOME, then XDG_DATA_DIRS (defaults per the XDG spec).
        let mut schema_dirs: Vec<PathBuf> = Vec::new();
        if let Some(dir) = env("GSETTINGS_SCHEMA_DIR") {
            schema_dirs.push(PathBuf::from(dir));
        }
        let home = env("HOME").map(PathBuf::from);
        let data_home = env("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home.clone().map(|h| h.join(".local/share")));
        if let Some(dir) = data_home {
            schema_dirs.push(dir.join("glib-2.0/schemas"));
        }
        let data_dirs =
            env("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        for dir in data_dirs.split(':').filter(|d| !d.is_empty()) {
            schema_dirs.push(PathBuf::from(dir).join("glib-2.0/schemas"));
        }

        // dconf locations under XDG_CONFIG_HOME (default $HOME/.config).
        let config_home = env("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home.clone().map(|h| h.join(".config")));
        let dconf_user_db = config_home.as_ref().map(|c| c.join("dconf/user").clone());
        let profile_name = env("DCONF_PROFILE").unwrap_or_else(|| "user".into());
        let profile_file = config_home
            .as_ref()
            .map(|c| c.join("dconf/profile").join(&profile_name));

        // System databases: profile entries resolve under /etc/dconf/db and
        // the XDG config dirs.
        let mut system_roots: Vec<PathBuf> = vec![PathBuf::from("/etc/dconf/db")];
        let config_dirs = env("XDG_CONFIG_DIRS").unwrap_or_else(|| "/etc/xdg".into());
        for dir in config_dirs.split(':').filter(|d| !d.is_empty()) {
            system_roots.push(PathBuf::from(dir).join("dconf/db"));
        }

        let mut notes = Vec::new();
        if let Some(backend) = env("GSETTINGS_BACKEND")
            && backend != "dconf"
        {
            notes.push(format!(
                "GSETTINGS_BACKEND={backend}: this session requests a non-dconf \
                 GSettings backend; values are read from the dconf databases and \
                 may not match what applications using that backend see."
            ));
        }

        let dconf_system_dbs = profile_file
            .and_then(|file| parse_dconf_profile(&file))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|name| {
                system_roots
                    .iter()
                    .map(|root| root.join(&name))
                    .find(|candidate| candidate.is_file())
            })
            .collect();

        Self {
            schema_dirs,
            dconf_user_db,
            dconf_system_dbs,
            notes,
        }
    }

    /// Fixture constructor: explicit roots, no environment reads.
    pub fn for_paths(
        schema_dirs: Vec<PathBuf>,
        dconf_user_db: Option<PathBuf>,
        dconf_system_dbs: Vec<PathBuf>,
    ) -> Self {
        Self {
            schema_dirs,
            dconf_user_db,
            dconf_system_dbs,
            notes: Vec::new(),
        }
    }

    /// Locate the first schema database (in priority order) that declares
    /// `schema`. Returns the parsed bytes plus the file path for details.
    fn load_schema_db(&self, schema: &str) -> Result<Option<(Vec<u8>, String)>, LicomanError> {
        for dir in &self.schema_dirs {
            let file = dir.join("gschemas.compiled");
            if !file.is_file() {
                continue;
            }
            let data = read_bounded(&file, MAX_DB_BYTES).map_err(|error| {
                LicomanError::new(
                    ErrorKind::Unavailable,
                    "The GSettings schema database could not be read.",
                    format!("{}: {error}", file.display()),
                )
            })?;
            match gvdb::has_schema(&data, schema) {
                Ok(true) => return Ok(Some((data, file.display().to_string()))),
                Ok(false) => {}
                Err(error) => {
                    return Err(LicomanError::new(
                        ErrorKind::Unavailable,
                        "The GSettings schema database could not be parsed.",
                        format!("{}: {error:?}", file.display()),
                    ));
                }
            }
        }
        Ok(None)
    }
}

/// Parse a dconf profile file: `user-db:<name>` / `system-db:<name>` lines
/// in priority order (user first per dconf's rule). Unknown/empty lines and
/// the `-` skip marker are ignored. Returns system db names only.
fn parse_dconf_profile(file: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(file).ok()?;
    let mut system = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix("system-db:") {
            let name = name.trim();
            if !name.is_empty() {
                system.push(name.to_string());
            }
        }
        // user-db entries map to the fixed user database, handled separately.
    }
    Some(system)
}

/// Handle to the native GSettings change watcher (07 Integration B: "Support
/// external changes via native settings notifications"). Dropping it stops
/// the thread.
pub struct NativeChangeWatch {
    cancel: gio::glib::Sender<()>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for NativeChangeWatch {
    fn drop(&mut self) {
        let _ = self.cancel.send(());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Subscribe to the native `changed` signals of `(schema, key)` pairs on a
/// dedicated thread with its own main context, forwarding each notification
/// to `notify` (the caller debounces). Schemas that are not installed are
/// skipped (they cannot change). Returns `None` when no GLib schema source
/// exists — Refresh remains the invalidation path, as with every watcher.
pub fn spawn_native_change_watcher(
    keys: Vec<(String, String)>,
    notify: impl Fn() + Send + Sync + 'static,
) -> Option<NativeChangeWatch> {
    if keys.is_empty() {
        return None;
    }
    let notify = std::sync::Arc::new(notify);
    // The deprecated MainContext channel is the cancel path only; glib's
    // suggested async-channel replacement is not otherwise a dependency.
    #[allow(deprecated)]
    let (cancel_tx, cancel_rx) =
        gio::glib::MainContext::channel::<()>(gio::glib::Priority::DEFAULT);
    let join = std::thread::Builder::new()
        .name("licoman-gsettings-watch".into())
        .spawn(move || {
            use gio::glib::ControlFlow;
            use gio::prelude::*;

            let context = gio::glib::MainContext::new();
            // Panic-safe thread-default scope: signal dispatch is bound to
            // this context, and it is popped when the closure returns.
            let _ = context.with_thread_default(|| {
                if let Some(source) = gio::SettingsSchemaSource::default() {
                    for (schema, key) in &keys {
                        let Some(schema_obj) = source.lookup(schema, true) else {
                            continue;
                        };
                        if !schema_obj.has_key(key) {
                            continue;
                        }
                        let settings = gio::Settings::new_full(
                            &schema_obj,
                            None::<&gio::SettingsBackend>,
                            schema_obj.path().as_deref(),
                        );
                        let notify = notify.clone();
                        settings.connect_changed(Some(key.as_str()), move |_settings, _key| {
                            notify();
                        });
                    }
                }

                let main_loop = gio::glib::MainLoop::new(Some(&context), false);
                {
                    let main_loop = main_loop.clone();
                    cancel_rx.attach(Some(&context), move |_| {
                        main_loop.quit();
                        ControlFlow::Break
                    });
                }
                main_loop.run();
            });
        })
        .ok()?;
    Some(NativeChangeWatch {
        cancel: cancel_tx,
        join: Some(join),
    })
}

fn value_error(id: &str, reason: impl Into<String>, technical: impl Into<String>) -> LicomanError {
    let technical = format!("{id}: {}", technical.into());
    LicomanError::new(ErrorKind::Unavailable, reason, technical)
}

impl SettingsReadBackend for GSettingsReadBackend {
    fn kind(&self) -> &'static str {
        "gsettings"
    }

    fn probe(&self, binding: &BackendBinding) -> SupportState {
        let BackendBinding::GSettings { schema, key } = binding else {
            return SupportState::Unavailable {
                reason: "Not a GSettings binding.".into(),
            };
        };
        let Some((data, file)) = self.load_schema_db(schema).ok().flatten() else {
            return SupportState::Unavailable {
                reason: format!(
                    "The GSettings schema {schema:?} is not installed (no compiled schema \
                     database in the searched locations contains it)."
                ),
            };
        };
        match gvdb::lookup_schema_value(&data, schema, key) {
            Ok(Some(_)) => {
                if self.dconf_user_db.is_none() {
                    SupportState::Degraded {
                        note: "No dconf user database found; only schema defaults can be shown."
                            .into(),
                    }
                } else {
                    match self.notes.len() {
                        0 => SupportState::Available,
                        _ => SupportState::Degraded {
                            note: self.notes.join(" "),
                        },
                    }
                }
            }
            Ok(None) => SupportState::Unavailable {
                reason: format!(
                    "The GSettings schema {schema:?} exists but declares no key {key:?}."
                ),
            },
            Err(error) => SupportState::Unavailable {
                reason: format!("The schema database at {file} could not be parsed: {error:?}."),
            },
        }
    }

    fn read(
        &self,
        binding: &BackendBinding,
        descriptor: &SettingDescriptor,
    ) -> Result<ReadResult, LicomanError> {
        let BackendBinding::GSettings { schema, key } = binding else {
            return Err(value_error(
                &descriptor.id,
                "This setting is not bound to GSettings.",
                "gsettings backend received a non-gsettings binding",
            ));
        };

        // 1. Schema database: default value + dconf path of the key.
        let Some((schema_data, schema_file)) = self.load_schema_db(schema)? else {
            return Err(value_error(
                &descriptor.id,
                format!(
                    "The GSettings schema {schema:?} is not installed on this system, so \
                     this setting has no value here."
                ),
                format!("no gschemas.compiled declares {schema:?}"),
            ));
        };
        let default_raw = gvdb::lookup_schema_value(&schema_data, schema, key)
            .map_err(|error| {
                value_error(
                    &descriptor.id,
                    "The GSettings schema database could not be parsed.",
                    format!("{schema:?}: {error:?}"),
                )
            })?
            .ok_or_else(|| {
                value_error(
                    &descriptor.id,
                    format!("The GSettings schema {schema:?} exists but declares no key {key:?}."),
                    format!("{schema:?}/{key} missing from {schema_file}"),
                )
            })?;
        let default_value = decode_framed(default_raw, true).map_err(|error| {
            value_error(
                &descriptor.id,
                "The schema default for this setting could not be decoded.",
                format!("{schema:?}/{key}: {error:?}"),
            )
        })?;

        // 2. Overrides: user db first (highest priority), then system dbs.
        let dconf_path = gvdb::lookup_schema_path(&schema_data, schema)
            .map_err(|error| {
                value_error(
                    &descriptor.id,
                    "The GSettings schema database could not be parsed.",
                    format!("{schema:?} .path: {error:?}"),
                )
            })?
            .map(|base| format!("{base}{key}"));

        let mut diagnostics = vec![format!("schema db: {schema_file}")];
        let mut override_hit: Option<(GVariantValue, ValueOrigin, String)> = None;
        if let Some(path) = &dconf_path {
            let mut dbs: Vec<(Option<&PathBuf>, ValueOrigin)> = Vec::new();
            if let Some(user_db) = &self.dconf_user_db {
                dbs.push((Some(user_db), ValueOrigin::User));
            }
            for system_db in &self.dconf_system_dbs {
                dbs.push((Some(system_db), ValueOrigin::Managed));
            }
            for (db_path, origin) in dbs {
                let Some(db_path) = db_path else { continue };
                if !db_path.is_file() {
                    continue;
                }
                let data = read_bounded(db_path, MAX_DB_BYTES).map_err(|error| {
                    value_error(
                        &descriptor.id,
                        "A dconf database could not be read.",
                        format!("{}: {error}", db_path.display()),
                    )
                })?;
                match gvdb::dconf_lookup(&data, path) {
                    Ok(Some(raw)) => {
                        let decoded = decode_framed(raw, false).map_err(|error| {
                            value_error(
                                &descriptor.id,
                                "The stored value of this setting could not be decoded.",
                                format!("dconf {path}: {error:?}"),
                            )
                        })?;
                        diagnostics.push(format!(
                            "override found in {} ({})",
                            db_path.display(),
                            match origin {
                                ValueOrigin::User => "user database",
                                _ => "system database",
                            }
                        ));
                        override_hit = Some((decoded, origin, db_path.display().to_string()));
                        break; // first hit in priority order wins
                    }
                    Ok(None) => {}
                    Err(error) => diagnostics
                        .push(format!("dconf db {} skipped: {error:?}", db_path.display())),
                }
            }
        } else {
            diagnostics.push(
                "relocatable schema (no .path): dconf overrides cannot be located; \
                 the schema default is shown"
                    .to_string(),
            );
        }
        for note in &self.notes {
            diagnostics.push(note.clone());
        }

        // 3. Compose: override else default; map to the descriptor's type.
        let (effective, origin, origin_note) = match &override_hit {
            Some((value, origin, db)) => (
                value.clone(),
                *origin,
                Some(match origin {
                    ValueOrigin::User => format!("dconf user database ({db})"),
                    _ => format!("dconf system database ({db})"),
                }),
            ),
            None => (
                default_value,
                ValueOrigin::Default,
                Some("schema default".to_string()),
            ),
        };
        let value = to_setting_value(&effective, descriptor)?;

        let mut fingerprint_bytes = Vec::new();
        fingerprint_bytes.extend_from_slice(schema.as_bytes());
        fingerprint_bytes.extend_from_slice(key.as_bytes());
        match &override_hit {
            Some((v, _, db)) => {
                fingerprint_bytes.extend_from_slice(db.as_bytes());
                fingerprint_bytes.extend_from_slice(format!("{v:?}").as_bytes());
            }
            None => {
                fingerprint_bytes.extend_from_slice(default_raw);
            }
        }

        Ok(ReadResult {
            value,
            origin,
            // Phase 05 is read-only; Phase 06 owns the write path.
            writable: Writable::No {
                reason: "Licoman can read GSettings but cannot change them yet.".to_string(),
            },
            freshness: Freshness::now(fnv1a_fingerprint(&fingerprint_bytes)),
            restart_effect: RestartEffect::Session,
            details: SourceDetails {
                backend_kind: "gsettings",
                target: format!("{schema} / {key}"),
                origin_note,
                imported: false,
                diagnostics,
            },
        })
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        self.dconf_user_db
            .iter()
            .filter(|p| p.is_file())
            .cloned()
            .collect()
    }
}

/// Map a decoded GVariant to the descriptor's declared type. A mismatch is
/// a typed error (test matrix §6.1), never a coercion.
fn to_setting_value(
    decoded: &GVariantValue,
    descriptor: &SettingDescriptor,
) -> Result<SettingValue, LicomanError> {
    use crate::value::SettingType;
    let mismatch = |technical: String| {
        Err(value_error(
            &descriptor.id,
            format!(
                "The stored value of {} is a different type than this setting declares.",
                descriptor.label
            ),
            technical,
        ))
    };
    Ok(match (decoded, descriptor.setting_type) {
        (GVariantValue::Bool(b), SettingType::Bool) => SettingValue::Bool(*b),
        (GVariantValue::Str(s), SettingType::Text) => SettingValue::Text(s.clone()),
        (GVariantValue::Str(s), SettingType::Choice) => SettingValue::Choice(s.clone()),
        (GVariantValue::Str(s), SettingType::Path) => SettingValue::Path(s.clone()),
        (GVariantValue::Int(n), SettingType::Integer) => SettingValue::Integer(*n),
        (GVariantValue::Uint(n), SettingType::Integer) => {
            SettingValue::Integer((*n).min(i64::MAX as u64) as i64)
        }
        (GVariantValue::Double(n), SettingType::Float) => SettingValue::Float(*n),
        (other, expected) => {
            return mismatch(format!(
                "gvariant {other:?} does not match declared type {}",
                expected.as_str()
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{Control, Scope};
    use std::path::PathBuf;

    fn fixtures_gsettings() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/gsettings")
    }

    fn schemas_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/discovery/schemas")
    }

    fn backend() -> GSettingsReadBackend {
        GSettingsReadBackend::for_paths(
            vec![schemas_dir()],
            Some(fixtures_gsettings().join("dconf/user")),
            vec![fixtures_gsettings().join("dconf/system/site")],
        )
    }

    fn descriptor_bool(id: &str) -> SettingDescriptor {
        SettingDescriptor::new(
            id,
            Scope::Desktop,
            "Test",
            "Test setting",
            "",
            crate::value::SettingType::Bool,
            Control::Switch,
        )
    }

    fn binding(schema: &str, key: &str) -> BackendBinding {
        BackendBinding::GSettings {
            schema: schema.into(),
            key: key.into(),
        }
    }

    #[test]
    fn effective_override_wins_over_schema_default() {
        let result = backend()
            .read(
                &binding("org.gnome.desktop.interface", "clock-show-weekday"),
                &descriptor_bool("t1"),
            )
            .unwrap();
        assert_eq!(result.value, SettingValue::Bool(true)); // dconf override
        assert_eq!(result.origin, ValueOrigin::User);
        assert!(
            result
                .details
                .origin_note
                .unwrap()
                .contains("user database")
        );
    }

    #[test]
    fn schema_default_used_when_no_override() {
        let result = backend()
            .read(
                &binding("org.gnome.desktop.interface", "clock-show-seconds"),
                &descriptor_bool("t2"),
            )
            .unwrap();
        assert_eq!(result.value, SettingValue::Bool(false));
        assert_eq!(result.origin, ValueOrigin::Default);
        assert_eq!(
            result.details.origin_note.as_deref(),
            Some("schema default")
        );
    }

    #[test]
    fn system_database_is_managed_origin() {
        let result = backend()
            .read(
                &binding("org.gnome.desktop.interface", "color-scheme"),
                &SettingDescriptor::new(
                    "t3",
                    Scope::Desktop,
                    "Test",
                    "Color scheme",
                    "",
                    crate::value::SettingType::Text,
                    Control::Text {
                        placeholder: String::new(),
                    },
                ),
            )
            .unwrap();
        assert_eq!(result.value, SettingValue::Text("prefer-dark".into()));
        assert_eq!(result.origin, ValueOrigin::Managed);
    }

    #[test]
    fn missing_key_and_schema_are_distinct_truthful_errors() {
        let error = backend()
            .read(
                &binding("org.gnome.desktop.interface", "no-such-key"),
                &descriptor_bool("t4"),
            )
            .unwrap_err();
        assert!(error.user_message.contains("declares no key"));
        let error = backend()
            .read(
                &binding("org.gnome.missing.schema", "key"),
                &descriptor_bool("t5"),
            )
            .unwrap_err();
        assert!(error.user_message.contains("not installed"));
    }

    #[test]
    fn typed_mismatch_is_reported_not_coerced() {
        // clock-format is a string; reading it as bool is a type error.
        let error = backend()
            .read(
                &binding("org.gnome.desktop.interface", "clock-format"),
                &descriptor_bool("t6"),
            )
            .unwrap_err();
        assert!(error.user_message.contains("different type"));
    }

    #[test]
    fn probe_reports_available_and_missing_states() {
        assert_eq!(
            backend().probe(&binding(
                "org.gnome.desktop.interface",
                "clock-show-weekday"
            )),
            SupportState::Available
        );
        assert!(matches!(
            backend().probe(&binding("org.gnome.missing", "key")),
            SupportState::Unavailable { .. }
        ));
    }

    #[test]
    fn watch_paths_cover_the_user_database() {
        assert_eq!(
            backend().watch_paths(),
            vec![fixtures_gsettings().join("dconf/user")]
        );
    }
}
