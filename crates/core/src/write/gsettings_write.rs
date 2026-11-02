//! The GSettings mutation path (07_FIRST_REAL_INTEGRATIONS.md Integration B;
//! decision D51) — the **native** GSettings API only:
//!
//! - Writes go through `g_settings_set_value`, resets through
//!   `g_settings_reset` (the platform's actual reset semantics — the key's
//!   override is removed so the schema/system default applies again). No
//!   shell tool, no `sh -c`, no direct dconf database editing.
//! - Every value is coerced to the **schema's own variant type** and checked
//!   against the schema's declared range (`range_check`) before any call; a
//!   mismatch is refused, never coerced into a wrong-type write.
//! - Verification reads back through the same native transport that wrote.
//! - The **desktop gate** (fail-closed): the engine refuses GSettings writes
//!   unless discovery positively identified a GNOME session. A schema being
//!   installed on a non-GNOME desktop never makes a desktop control
//!   functional here (07 Integration B).
//!
//! Row reads keep the self-contained GVDB parser (Phase 05) so fixture tests
//! stay independent of a desktop session; the native layer is used for the
//! write path, its verification, and the native change notifications.

use std::sync::{Arc, Mutex};

use gio::glib::ToVariant as _;
use gio::prelude::*;

use crate::descriptor::SettingDescriptor;
use crate::error::{ErrorKind, LicomanError};
use crate::value::SettingValue;
use crate::write::request::{Change, ChangeRequest, WriteTarget};
use crate::write::{ApplyOutcome, Persistence, conflict};

/// Shared, refreshable desktop-gate state. `None` = unknown (closed);
/// `Some(true)` only after discovery positively identified a GNOME session.
#[derive(Clone, Default)]
pub struct GSettingsDesktopGate {
    state: Arc<Mutex<Option<bool>>>,
}

impl GSettingsDesktopGate {
    /// Closed by default (fail-closed on unknown).
    pub fn closed() -> Self {
        Self::default()
    }

    /// Update from a discovery verdict.
    pub fn set(&self, gnome_session: Option<bool>) {
        *self.state.lock().expect("gsettings gate lock") = gnome_session;
    }

    pub fn get(&self) -> Option<bool> {
        *self.state.lock().expect("gsettings gate lock")
    }
}

/// What the native surface can fail with. Distinct causes keep the honest
/// states apart (missing session vs missing schema vs refusal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GsClientError {
    /// No default schema source (no GLib schema database readable).
    NoSchemaSource,
    /// The schema or the exact key does not exist.
    NoSchemaOrKey(String),
    /// The value cannot be represented in the schema's declared type, or is
    /// outside the schema's declared range.
    ValueRejected(String),
    /// The platform refused the write (read-only key, locked backend).
    Denied(String),
    /// Anything else, with a technical detail.
    Other(String),
}

impl GsClientError {
    /// User-safe mapping for the provider's verification path.
    pub fn to_licoman(&self, what: &str) -> LicomanError {
        let technical = format!("{what}: {self:?}");
        match self {
            GsClientError::NoSchemaSource | GsClientError::NoSchemaOrKey(_) => LicomanError::new(
                ErrorKind::Unavailable,
                "The GSettings schema for this setting is not available, so it \
                     cannot be changed.",
                technical,
            ),
            GsClientError::ValueRejected(_) => LicomanError::new(
                ErrorKind::Validation,
                "This value is not valid for the setting's schema. Nothing was \
                 modified.",
                technical,
            ),
            GsClientError::Denied(_) => LicomanError::new(
                ErrorKind::Permission,
                "The platform refused this change. Nothing was modified.",
                technical,
            ),
            GsClientError::Other(_) => LicomanError::new(
                ErrorKind::WriteFailed,
                "The GSettings change could not be applied. Nothing was modified.",
                technical,
            ),
        }
    }
}

/// The native GSettings surface (libgio). Tests inject an in-process
/// implementation over a memory backend and fixture schemas — no real dconf
/// database is ever touched by tests.
pub trait NativeGSettings: Send + Sync {
    /// Read `(effective value, user override)`; `None` for the override
    /// means the schema/system default applies.
    fn read(
        &self,
        schema: &str,
        key: &str,
    ) -> Result<(SettingValue, Option<SettingValue>), GsClientError>;

    /// Write one typed value (coerced and range-checked by the native side).
    fn write(&self, schema: &str, key: &str, value: &SettingValue) -> Result<(), GsClientError>;

    /// Platform reset: remove the user override so the default applies.
    fn reset(&self, schema: &str, key: &str) -> Result<(), GsClientError>;
}

/// The real native layer over the `gio` crate.
pub struct GioNativeGSettings;

impl NativeGSettings for GioNativeGSettings {
    fn read(
        &self,
        schema: &str,
        key: &str,
    ) -> Result<(SettingValue, Option<SettingValue>), GsClientError> {
        let settings = open(schema, key)?;
        let effective = variant_to_value(&settings.value(key))?;
        let user = settings
            .user_value(key)
            .map(|variant| variant_to_value(&variant))
            .transpose()?;
        Ok((effective, user))
    }

    fn write(&self, schema: &str, key: &str, value: &SettingValue) -> Result<(), GsClientError> {
        let settings = open(schema, key)?;
        let source = gio::SettingsSchemaSource::default().ok_or(GsClientError::NoSchemaSource)?;
        let schema_obj = lookup(&source, schema)?;
        let schema_key = schema_obj.key(key);
        let variant = coerce_to_variant(value, &schema_key)?;
        if !schema_key.range_check(&variant) {
            return Err(GsClientError::ValueRejected(format!(
                "{schema}/{key}: value {value:?} is outside the schema's declared range"
            )));
        }
        settings.set_value(key, &variant).map_err(|error| {
            GsClientError::Denied(format!("g_settings_set_value refused: {error}"))
        })?;
        // dconf-level writes are queued in the client; without an explicit
        // sync they reach dconf-service (and every other reader) only at a
        // flush point, while this process's own readback would see its
        // client-local view — a false self-verification. g_settings_sync
        // pushes the queue so the readback below is service-authoritative.
        gio::Settings::sync();
        Ok(())
    }

    fn reset(&self, schema: &str, key: &str) -> Result<(), GsClientError> {
        let settings = open(schema, key)?;
        settings.reset(key);
        // Same queue semantics as write: sync before anyone reads back.
        gio::Settings::sync();
        Ok(())
    }
}

/// Open a `Settings` object for a schema that is verified to exist (and to
/// declare the key). `g_settings_new` aborts the process on an unknown
/// schema, so the lookup always goes through the schema source first.
fn open(schema: &str, key: &str) -> Result<gio::Settings, GsClientError> {
    let source = gio::SettingsSchemaSource::default().ok_or(GsClientError::NoSchemaSource)?;
    let schema_obj = lookup(&source, schema)?;
    Ok(gio::Settings::new_full(
        &schema_obj,
        None::<&gio::SettingsBackend>,
        schema_obj.path().as_deref(),
    ))
    .and_then(|settings| {
        if !schema_obj.has_key(key) {
            return Err(GsClientError::NoSchemaOrKey(format!(
                "{schema} declares no key {key:?}"
            )));
        }
        Ok(settings)
    })
}

fn lookup(
    source: &gio::SettingsSchemaSource,
    schema: &str,
) -> Result<gio::SettingsSchema, GsClientError> {
    source
        .lookup(schema, true)
        .ok_or_else(|| GsClientError::NoSchemaOrKey(format!("schema {schema:?} is not installed")))
}

/// Coerce a Licoman value into the schema key's declared variant type. A
/// mismatch is a refusal, never a wrong-type write.
fn coerce_to_variant(
    value: &SettingValue,
    schema_key: &gio::SettingsSchemaKey,
) -> Result<gio::glib::Variant, GsClientError> {
    let ty = schema_key.value_type();
    let reject = |why: String| GsClientError::ValueRejected(why);
    match ty.as_str() {
        "b" => match value {
            SettingValue::Bool(b) => Ok(b.to_variant()),
            other => Err(reject(format!("schema wants a boolean, got {other:?}"))),
        },
        "i" | "x" => match value {
            SettingValue::Integer(n) => Ok((*n).to_variant()),
            other => Err(reject(format!(
                "schema wants a signed integer, got {other:?}"
            ))),
        },
        "u" | "y" | "n" | "q" | "t" => match value {
            SettingValue::Integer(n) if *n >= 0 => Ok((*n as u64).to_variant()),
            other => Err(reject(format!(
                "schema wants an unsigned integer, got {other:?}"
            ))),
        },
        "d" => match value {
            SettingValue::Float(f) => Ok(f.to_variant()),
            SettingValue::Integer(n) => Ok((*n as f64).to_variant()),
            other => Err(reject(format!("schema wants a double, got {other:?}"))),
        },
        "s" | "o" => match value {
            SettingValue::Choice(text) | SettingValue::Text(text) | SettingValue::Path(text) => {
                Ok(text.to_variant())
            }
            other => Err(reject(format!("schema wants a string, got {other:?}"))),
        },
        other => Err(reject(format!(
            "schema type {other:?} is not supported for writes"
        ))),
    }
}

/// Decode a variant into a Licoman value by its own type.
fn variant_to_value(variant: &gio::glib::Variant) -> Result<SettingValue, GsClientError> {
    Ok(match variant.type_().as_str() {
        "b" => SettingValue::Bool(variant.get::<bool>().unwrap_or_default()),
        "y" | "n" | "q" | "u" | "t" => {
            SettingValue::Integer(variant.get::<u64>().unwrap_or_default() as i64)
        }
        "i" | "x" => SettingValue::Integer(variant.get::<i64>().unwrap_or_default()),
        "d" => SettingValue::Float(variant.get::<f64>().unwrap_or_default()),
        "s" | "o" => SettingValue::Text(variant.get::<String>().unwrap_or_default()),
        other => {
            return Err(GsClientError::Other(format!(
                "unsupported variant type {other:?}"
            )));
        }
    })
}

/// The GSettings write backend: desktop gate → semantic pre-check → typed
/// native write/reset → readback through the same native transport.
pub struct GSettingsWriteBackend {
    gate: GSettingsDesktopGate,
    native: Arc<dyn NativeGSettings>,
}

impl GSettingsWriteBackend {
    /// The production backend (real libgio, gate closed until discovery
    /// reports a GNOME session).
    pub fn native(gate: GSettingsDesktopGate) -> Self {
        Self {
            gate,
            native: Arc::new(GioNativeGSettings),
        }
    }

    /// Injectable native surface (tests: memory backend + fixture schemas).
    pub fn with_native(gate: GSettingsDesktopGate, native: Arc<dyn NativeGSettings>) -> Self {
        Self { gate, native }
    }

    /// The gate handle (the provider refreshes it from discovery).
    pub fn gate(&self) -> &GSettingsDesktopGate {
        &self.gate
    }

    /// One read through the native transport (the provider's verification
    /// path for GSettings writes: verification always uses the transport
    /// that wrote, like service targets — the dconf user-db file can lag
    /// behind dconf-service).
    pub fn native_read(
        &self,
        schema: &str,
        key: &str,
    ) -> Result<(SettingValue, Option<SettingValue>), GsClientError> {
        self.native.read(schema, key)
    }

    /// Whether writes are currently permitted (gate open) — used by the
    /// provider when presenting writability, and enforced here at apply.
    pub fn permitted(&self) -> bool {
        self.gate.get() == Some(true)
    }

    pub(crate) fn apply(
        &self,
        request: &ChangeRequest,
        _descriptor: &SettingDescriptor,
    ) -> Result<ApplyOutcome, LicomanError> {
        let (schema, key) = match &request.target {
            WriteTarget::GSettings { schema, key } => (schema, key),
            other => {
                return Err(LicomanError::new(
                    ErrorKind::WriteFailed,
                    "This change is not bound to a GSettings schema/key.",
                    format!(
                        "gsettings backend got target kind {:?} for {}",
                        other.kind_name(),
                        request.setting_id
                    ),
                ));
            }
        };
        if schema.is_empty() || key.is_empty() {
            return Err(LicomanError::new(
                ErrorKind::Validation,
                "This setting does not name a GSettings schema and key, so it cannot \
                 be written.",
                format!(
                    "gsettings target without schema/key for {}",
                    request.setting_id
                ),
            ));
        }

        // --- desktop gate (fail-closed, D51) -------------------------------
        match self.gate.get() {
            Some(true) => {}
            Some(false) => {
                return Err(LicomanError::new(
                    ErrorKind::Permission,
                    "This desktop is not GNOME, so Licoman does not present GNOME \
                     interface settings as writable here. Nothing was modified.",
                    format!(
                        "gsettings desktop gate closed for {} (schema {schema}/{key})",
                        request.setting_id
                    ),
                ));
            }
            None => {
                return Err(LicomanError::new(
                    ErrorKind::Permission,
                    "The desktop environment could not be determined, so GNOME \
                     settings are not writable. Nothing was modified.",
                    format!(
                        "gsettings desktop gate unknown for {} (schema {schema}/{key})",
                        request.setting_id
                    ),
                ));
            }
        }

        let what = format!("{} {schema}/{key}", request.setting_id);
        match &request.change {
            Change::Set(value) => {
                // --- semantic pre-check (fingerprint-free, like service
                // targets): the source must still hold what the UI saw.
                let (current, _user) = self
                    .native
                    .read(schema, key)
                    .map_err(|error| error.to_licoman(&what))?;
                match &request.before {
                    Some(before) if before == &current => {}
                    Some(before) => {
                        return Err(conflict(
                            &request.setting_id,
                            format!("{schema}/{key} holds {current:?}, expected {before:?}"),
                        ));
                    }
                    None => {
                        return Err(conflict(
                            &request.setting_id,
                            format!(
                                "{schema}/{key} holds {current:?}; it was not read \
                                     before staging"
                            ),
                        ));
                    }
                }
                self.native
                    .write(schema, key, value)
                    .map_err(|error| error.to_licoman(&what))?;
                // --- readback: the platform is authoritative ----------------
                let (after, _user) = self
                    .native
                    .read(schema, key)
                    .map_err(|error| error.to_licoman("verifying after the write"))?;
                let verified = after == *value;
                crate::log::info(
                    "core::write::gsettings",
                    "gsettings change applied",
                    &[
                        ("id", request.setting_id.to_string()),
                        ("verified", verified.to_string()),
                    ],
                );
                Ok(ApplyOutcome {
                    verified,
                    expect_absent: false,
                    persistence: Persistence::Persistent,
                    restart: request.restart,
                    readback_mismatch: (!verified).then_some(after),
                    notes: vec![format!("gsettings {schema}/{key}")],
                })
            }
            Change::RemoveOverride => {
                // Real reset semantics: remove the user override so the
                // schema/system default applies again (D51). If there is no
                // override, the desired state already holds.
                let (_current, user) = self
                    .native
                    .read(schema, key)
                    .map_err(|error| error.to_licoman(&what))?;
                if user.is_some() {
                    self.native
                        .reset(schema, key)
                        .map_err(|error| error.to_licoman(&what))?;
                }
                let (_after, user_after) = self
                    .native
                    .read(schema, key)
                    .map_err(|error| error.to_licoman("verifying after the reset"))?;
                let verified = user_after.is_none();
                crate::log::info(
                    "core::write::gsettings",
                    "gsettings reset applied",
                    &[
                        ("id", request.setting_id.to_string()),
                        ("verified", verified.to_string()),
                        ("was_noop", user.is_none().to_string()),
                    ],
                );
                // expect_absent marks a RemoveOverride that the backend
                // verified; the store then fills the row from the fresh
                // verification read (the schema default), never a fake
                // "absent" state.
                Ok(ApplyOutcome {
                    verified,
                    expect_absent: true,
                    persistence: Persistence::Persistent,
                    restart: request.restart,
                    readback_mismatch: None,
                    notes: vec![format!("gsettings reset {schema}/{key}")],
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{Control, Scope};
    use crate::value::SettingType;
    use crate::write::{RollbackStrategy, ServiceBus, ServiceEndpoint, WritePolicy};

    /// In-process native over a gio memory backend + the committed fixture
    /// schemas. Nothing here touches a real dconf database: the backend is
    /// per-process memory, the schemas are the test fixtures.
    struct MemoryNative {
        backend: gio::SettingsBackend,
        source: gio::SettingsSchemaSource,
    }

    impl MemoryNative {
        fn fixture() -> Self {
            let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/discovery/schemas");
            let source = gio::SettingsSchemaSource::from_directory(&dir, None, true)
                .expect("fixture schema source loads");
            Self {
                backend: gio::memory_settings_backend_new(),
                source,
            }
        }

        fn settings(&self, schema: &str) -> Result<gio::Settings, GsClientError> {
            let schema_obj = lookup(&self.source, schema)?;
            Ok(gio::Settings::new_full(
                &schema_obj,
                Some(&self.backend),
                schema_obj.path().as_deref(),
            ))
        }
    }

    // SAFETY: the held gio objects (GSettingsBackend, GSettingsSchemaSource)
    // are GLib objects with atomic refcounts and documented thread-safe
    // accessors; this struct only ever calls their thread-safe functions
    // through shared references. gio's bindings conservatively omit
    // Send/Sync; the test fixture below needs them for the trait bound.
    unsafe impl Send for MemoryNative {}
    unsafe impl Sync for MemoryNative {}

    impl NativeGSettings for MemoryNative {
        fn read(
            &self,
            schema: &str,
            key: &str,
        ) -> Result<(SettingValue, Option<SettingValue>), GsClientError> {
            let settings = self.settings(schema)?;
            if !lookup(&self.source, schema)?.has_key(key) {
                return Err(GsClientError::NoSchemaOrKey(format!("{schema}/{key}")));
            }
            let effective = variant_to_value(&settings.value(key))?;
            let user = settings
                .user_value(key)
                .map(|variant| variant_to_value(&variant))
                .transpose()?;
            Ok((effective, user))
        }

        fn write(
            &self,
            schema: &str,
            key: &str,
            value: &SettingValue,
        ) -> Result<(), GsClientError> {
            let settings = self.settings(schema)?;
            let source = &self.source;
            let schema_obj = lookup(source, schema)?;
            let schema_key = schema_obj.key(key);
            let variant = coerce_to_variant(value, &schema_key)?;
            if !schema_key.range_check(&variant) {
                return Err(GsClientError::ValueRejected("outside range".into()));
            }
            settings.set_value(key, &variant).map_err(|error| {
                GsClientError::Denied(format!("g_settings_set_value refused: {error}"))
            })?;
            Ok(())
        }

        fn reset(&self, schema: &str, key: &str) -> Result<(), GsClientError> {
            self.settings(schema)?.reset(key);
            Ok(())
        }
    }

    fn gate(open: bool) -> GSettingsDesktopGate {
        let gate = GSettingsDesktopGate::closed();
        gate.set(Some(open));
        gate
    }

    fn backend(open: bool) -> GSettingsWriteBackend {
        GSettingsWriteBackend::with_native(gate(open), Arc::new(MemoryNative::fixture()))
    }

    fn descriptor() -> SettingDescriptor {
        SettingDescriptor::new(
            "desktop.gnome.interface.clock.show_weekday",
            Scope::Desktop,
            "Test",
            "Show weekday",
            "",
            SettingType::Bool,
            Control::Switch,
        )
    }

    fn request(change: Change, before: Option<SettingValue>) -> ChangeRequest {
        ChangeRequest {
            setting_id: "desktop.gnome.interface.clock.show_weekday".into(),
            module_id: "desktop.gnome.interface".into(),
            target: WriteTarget::GSettings {
                schema: "org.gnome.desktop.interface".into(),
                key: "clock-show-weekday".into(),
            },
            before,
            change,
            expected_fingerprint: None,
            risk: crate::modules::def::RiskLevel::Low,
            policy: WritePolicy::Immediate,
            restart: crate::read::RestartEffect::Session,
            rollback: RollbackStrategy::InverseValue,
        }
    }

    #[test]
    fn write_readback_and_native_reset_round_trip() {
        let backend = backend(true);
        // Pre-state: fixture schema default for clock-show-weekday is false.
        let (current, user) = backend
            .native
            .read("org.gnome.desktop.interface", "clock-show-weekday")
            .unwrap();
        assert_eq!(current, SettingValue::Bool(false));
        assert_eq!(user, None);

        let outcome = backend
            .apply(
                &request(Change::Set(SettingValue::Bool(true)), Some(current.clone())),
                &descriptor(),
            )
            .unwrap();
        assert!(outcome.verified);
        assert_eq!(outcome.persistence, Persistence::Persistent);

        // The native layer genuinely holds the new value; the user override
        // exists now.
        let (after, user) = backend
            .native
            .read("org.gnome.desktop.interface", "clock-show-weekday")
            .unwrap();
        assert_eq!(after, SettingValue::Bool(true));
        assert_eq!(user, Some(SettingValue::Bool(true)));

        // Reset removes the override; the schema default applies again.
        let outcome = backend
            .apply(&request(Change::RemoveOverride, Some(after)), &descriptor())
            .unwrap();
        assert!(outcome.verified);
        assert!(outcome.expect_absent);
        let (after, user) = backend
            .native
            .read("org.gnome.desktop.interface", "clock-show-weekday")
            .unwrap();
        assert_eq!(after, SettingValue::Bool(false));
        assert_eq!(user, None);
    }

    #[test]
    fn gate_closed_and_unknown_refuse_without_touching_anything() {
        let closed = backend(false);
        let error = closed
            .apply(
                &request(
                    Change::Set(SettingValue::Bool(true)),
                    Some(SettingValue::Bool(false)),
                ),
                &descriptor(),
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Permission);
        assert!(error.user_message.contains("not GNOME"));

        let unknown = GSettingsWriteBackend::with_native(
            GSettingsDesktopGate::closed(),
            Arc::new(MemoryNative::fixture()),
        );
        assert!(unknown.gate.get().is_none());
        let error = unknown
            .apply(
                &request(
                    Change::Set(SettingValue::Bool(true)),
                    Some(SettingValue::Bool(false)),
                ),
                &descriptor(),
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Permission);
        // Nothing was written in either case.
        let (current, user) = closed
            .native
            .read("org.gnome.desktop.interface", "clock-show-weekday")
            .unwrap();
        assert_eq!(current, SettingValue::Bool(false));
        assert_eq!(user, None);
    }

    #[test]
    fn external_change_before_apply_is_a_conflict() {
        let backend = backend(true);
        // The UI saw `false`; an external tool then sets the key to true.
        backend
            .native
            .write(
                "org.gnome.desktop.interface",
                "clock-show-weekday",
                &SettingValue::Bool(true),
            )
            .unwrap();
        // The stale intent must conflict, not overwrite.
        let error = backend
            .apply(
                &request(
                    Change::Set(SettingValue::Bool(true)),
                    Some(SettingValue::Bool(false)),
                ),
                &descriptor(),
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Conflict);
        let (current, user) = backend
            .native
            .read("org.gnome.desktop.interface", "clock-show-weekday")
            .unwrap();
        assert_eq!(current, SettingValue::Bool(true));
        assert_eq!(user, Some(SettingValue::Bool(true)));
    }

    #[test]
    fn type_mismatch_is_refused_before_any_write() {
        let backend = backend(true);
        let error = backend
            .native
            .write(
                "org.gnome.desktop.interface",
                "clock-show-weekday",
                &SettingValue::Text("yes".into()),
            )
            .unwrap_err();
        assert!(matches!(error, GsClientError::ValueRejected(_)));
        let (current, _) = backend
            .native
            .read("org.gnome.desktop.interface", "clock-show-weekday")
            .unwrap();
        assert_eq!(current, SettingValue::Bool(false));
    }

    #[test]
    fn absent_schema_and_key_are_distinct_honest_errors() {
        let backend = backend(true);
        let error = backend.native.read("org.gnome.missing", "key").unwrap_err();
        assert!(matches!(error, GsClientError::NoSchemaOrKey(_)));
        let error = backend
            .native
            .read("org.gnome.desktop.interface", "no-such-key")
            .unwrap_err();
        assert!(matches!(error, GsClientError::NoSchemaOrKey(_)));
    }

    #[test]
    fn non_gsettings_targets_are_refused() {
        let backend = backend(true);
        let request = ChangeRequest {
            setting_id: "shared.power.profile".into(),
            module_id: "shared.power".into(),
            target: WriteTarget::Service {
                endpoint: ServiceEndpoint {
                    bus: ServiceBus::System,
                    service: "org.freedesktop.UPower.PowerProfiles".into(),
                    path: "/org/freedesktop/UPower/PowerProfiles".into(),
                    interface: "org.freedesktop.UPower.PowerProfiles".into(),
                },
                property: "ActiveProfile".into(),
            },
            before: Some(SettingValue::Choice("balanced".into())),
            change: Change::Set(SettingValue::Choice("balanced".into())),
            expected_fingerprint: None,
            risk: crate::modules::def::RiskLevel::Low,
            policy: WritePolicy::Staged,
            restart: crate::read::RestartEffect::None,
            rollback: RollbackStrategy::InverseValue,
        };
        let error = backend.apply(&request, &descriptor()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::WriteFailed);
    }
}
