//! The provider bridge: exposes the module registry through the existing
//! [`Provider`] trait so the Phase 02 [`crate::model::Store`], command
//! vocabulary and rendering paths keep working unchanged
//! (04_MODULE_SCHEMA_AND_REGISTRY.md §4).
//!
//! Read/write rules, fail-closed:
//!
//! - **mock** bindings serve declared defaults (never invented values) and
//!   accept in-memory writes so staged/immediate flows remain exercisable.
//! - **gsettings / dbus / file** bindings read through the Phase 05 backend
//!   router (05_READ_ONLY_BACKENDS.md): typed values with origin and
//!   freshness, bounded I/O, background latency. Writes are refused
//!   unconditionally — the write engine is Phase 06.
//! - `read_only` policy and `required_auth` settings refuse writes with
//!   distinct, user-safe errors.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::def::ApplyPolicy;
use super::def::BackendBinding;
use super::registry::ModuleRegistry;
use crate::descriptor::SettingDescriptor;
use crate::error::{ErrorKind, LicomanError};
use crate::modules::def::ValueKind;
use crate::provider::{Capabilities, ProbeResult, Provider, ReadLatency};
use crate::read::{BackendRouter, ReadResult, SourceDetails, ValueOrigin, Writable};
use crate::value::SettingValue;

/// Mock-backed settings whose writes are rejected to exercise the visible
/// failure flow (same deterministic behavior the Phase 02 fixture catalog
/// had). Purely an in-memory mock concern; real backends refuse anyway.
const MOCK_FAIL_WRITES: &[&str] = &["shared.network.dns"];

/// The per-setting facts the read/write paths need from the registry.
type SettingFacts = (
    crate::modules::def::BackendBinding,
    Option<ValueKind>,
    Option<super::def::ManifestValue>,
    bool,
    ApplyPolicy,
    Option<String>,
);

/// Cheap to clone: the adapter holds one handle (to refresh the environment
/// and read module metadata) while another clone is registered as the
/// `Provider` behind the core `Registry`.
#[derive(Clone)]
pub struct ModuleProvider {
    registry: Arc<Mutex<ModuleRegistry>>,
    written: Arc<Mutex<HashMap<String, SettingValue>>>,
    router: BackendRouter,
    /// The write engine (Phase 06 fixtures / Phase 07 live, D49). `None`
    /// means every non-mock write is refused.
    engine: Option<std::sync::Arc<crate::write::WriteEngine>>,
    /// Shared handle to the engine's GSettings desktop gate, when a
    /// GSettings backend is attached; refreshed from discovery snapshots.
    gsettings_gate: Option<crate::write::GSettingsDesktopGate>,
}

impl ModuleProvider {
    /// Wrap a registry with a live backend router.
    pub fn new(registry: Arc<Mutex<ModuleRegistry>>) -> Self {
        Self::with_router(registry, BackendRouter::live())
    }

    /// Phase 07 live constructor (D49): the live router **plus** a real
    /// write engine — file writes over the live approved roots with private
    /// backups under the Licoman data dir, service writes through the real
    /// zbus client, GSettings writes through the native API behind the
    /// fail-closed desktop gate. Nothing is written by construction: every
    /// write still requires the user's explicit Apply/Confirm flow, and
    /// tests never call this.
    pub fn live(registry: Arc<Mutex<ModuleRegistry>>) -> Self {
        let router = BackendRouter::live();
        let file_write = crate::write::FileWriteBackend::new(
            crate::read::file::FileReadBackend::from_env(),
            crate::prefs::data_dir().join("backups"),
        );
        let service_write = crate::write::ServiceWriteBackend::new(Arc::new(
            crate::read::dbus::ZBusPowerProfilesClient::default(),
        ));
        let gsettings = crate::write::GSettingsWriteBackend::native(
            crate::write::GSettingsDesktopGate::closed(),
        );
        let engine = crate::write::WriteEngine::new(file_write, service_write)
            .with_gsettings(std::sync::Arc::new(gsettings));
        Self::with_write_engine(registry, router, engine)
    }

    /// Wrap a registry with an explicit backend router (fixtures, demos).
    pub fn with_router(registry: Arc<Mutex<ModuleRegistry>>, router: BackendRouter) -> Self {
        Self {
            registry,
            written: Arc::new(Mutex::new(HashMap::new())),
            router,
            engine: None,
            gsettings_gate: None,
        }
    }

    /// Attach a write engine and keep the natural (editable) controls
    /// (D49). Shared by the fixture construction (Phase 06 demos/tests,
    /// injected roots) and the live constructor (Phase 07); the engine's
    /// gates do the enforcing either way.
    pub fn with_write_engine(
        registry: Arc<Mutex<ModuleRegistry>>,
        router: BackendRouter,
        engine: crate::write::WriteEngine,
    ) -> Self {
        {
            let mut locked = registry.lock().expect("registry lock");
            locked.enable_write_controls();
        }
        let gsettings_gate = engine.gsettings().map(|backend| backend.gate().clone());
        Self {
            registry,
            written: Arc::new(Mutex::new(HashMap::new())),
            router,
            engine: Some(std::sync::Arc::new(engine)),
            gsettings_gate,
        }
    }

    /// Phase 06 fixture-write mode: route non-mock writes through the safe
    /// write engine (file/service backends over **injected fixture roots**,
    /// deny-by-default privilege broker). The registry's descriptors keep
    /// their natural controls so the flows are exercisable end to end.
    /// Live environments use [`ModuleProvider::live`].
    pub fn with_fixture_write_engine(
        registry: Arc<Mutex<ModuleRegistry>>,
        router: BackendRouter,
        engine: crate::write::WriteEngine,
    ) -> Self {
        Self::with_write_engine(registry, router, engine)
    }

    /// Re-evaluate applicability against a snapshot. Returns whether the set
    /// of available settings changed (the adapter reloads only then).
    /// Also refreshes the GSettings desktop gate from the detected session
    /// (D51): only a positively identified GNOME session opens it.
    pub fn set_environment(
        &self,
        snapshot: Option<&crate::discovery::EnvironmentSnapshot>,
    ) -> bool {
        let gnome = snapshot
            .map(|snapshot| snapshot.session.desktop == crate::discovery::DesktopIdentity::Gnome);
        if let Some(gate) = &self.gsettings_gate {
            gate.set(gnome);
            crate::log::info(
                "core::modules",
                "gsettings desktop gate refreshed",
                &[("open", gnome.unwrap_or(false).to_string())],
            );
        }
        let mut registry = self.registry.lock().expect("registry lock");
        registry.set_environment(snapshot)
    }

    /// Read access to the registry for the adapter (module lists, provenance,
    /// app summaries). Short critical sections only — never held across
    /// rendering or awaits.
    pub fn with_registry<R>(&self, read: impl FnOnce(&ModuleRegistry) -> R) -> R {
        let registry = self.registry.lock().expect("registry lock");
        read(&registry)
    }

    /// The backend router handle (watch paths for the settings watcher).
    pub fn router(&self) -> &BackendRouter {
        &self.router
    }

    /// The (schema, key) pairs for the native GSettings change watcher
    /// (07 Integration B): every registered, currently-applicable
    /// gsettings-backed setting.
    pub fn gsettings_watch_keys(&self) -> Vec<(String, String)> {
        self.with_registry(|registry| {
            let ids: Vec<String> = registry
                .descriptors()
                .iter()
                .map(|descriptor| descriptor.id.clone())
                .collect();
            ids.iter()
                .filter_map(|id| {
                    registry.setting_status(id).ok()?;
                    let (_def, setting) = registry.setting(id)?;
                    match &setting.backend {
                        BackendBinding::GSettings { schema, key } => {
                            Some((schema.clone(), key.clone()))
                        }
                        _ => None,
                    }
                })
                .collect()
        })
    }

    /// Whether any registered, currently-applicable setting is bound to an
    /// Xfconf channel (drives the UI's signal watcher attachment).
    pub fn has_xfconf_settings(&self) -> bool {
        self.with_registry(|registry| {
            registry.descriptors().iter().any(|descriptor| {
                registry
                    .setting(&descriptor.id)
                    .is_some_and(|(_def, setting)| {
                        matches!(setting.backend, BackendBinding::Xfconf { .. })
                    })
            })
        })
    }

    fn setting_error(&self, id: &str, reason: String) -> LicomanError {
        LicomanError::new(
            ErrorKind::Unavailable,
            reason,
            format!("module registry status for {id:?}"),
        )
    }

    /// The (backend, kind, default, nullable, policy, auth) tuple for one
    /// registered, available setting.
    fn setting_facts(&self, id: &str) -> Result<SettingFacts, LicomanError> {
        {
            let registry = self.registry.lock().expect("registry lock");
            registry
                .setting_status(id)
                .map_err(|reason| self.setting_error(id, reason))?;
            let (_def, setting) = registry
                .setting(id)
                .ok_or_else(|| self.setting_error(id, "This setting is not registered.".into()))?;
            Ok((
                setting.backend.clone(),
                setting.kind,
                setting.default.clone(),
                setting.nullable,
                setting.policy,
                setting.required_auth.clone(),
            ))
        }
    }

    /// The mock value source (declared defaults, then staged writes).
    fn read_mock(
        &self,
        id: &str,
        kind: Option<ValueKind>,
        default: Option<super::def::ManifestValue>,
        nullable: bool,
    ) -> Result<SettingValue, LicomanError> {
        if let Some(written) = self.written.lock().expect("written lock").get(id) {
            return Ok(written.clone());
        }
        if let Some(default) = &default {
            let kind = kind.expect("validated: non-action settings declare a type");
            return super::registry::manifest_value_to_setting_value(default, kind).ok_or_else(
                || {
                    LicomanError::new(
                        ErrorKind::Unavailable,
                        "The declared default value does not match this setting's type.",
                        format!("default coercion failed for {id:?}"),
                    )
                },
            );
        }
        if nullable {
            return Ok(match kind {
                Some(ValueKind::Path) => SettingValue::Path(String::new()),
                Some(ValueKind::List) => SettingValue::List(Vec::new()),
                _ => SettingValue::Text(String::new()),
            });
        }
        Err(LicomanError::new(
            ErrorKind::Unavailable,
            "This setting has no value yet: no default is declared and mock reads have \
             nothing to serve.",
            format!("mock binding for {id:?} has no default"),
        ))
    }
}

impl Provider for ModuleProvider {
    fn id(&self) -> &'static str {
        "module-registry"
    }

    fn probe(&self) -> ProbeResult {
        // Read is real (mock value source + Phase 05 backends); write is
        // advertised because mock-backed settings accept in-memory writes —
        // per-setting gating happens in [`Provider::write`].
        ProbeResult::available(Capabilities {
            read: true,
            write: true,
            watch: true,
        })
    }

    fn descriptors(&self) -> Vec<SettingDescriptor> {
        self.with_registry(|registry| registry.descriptors().to_vec())
    }

    fn read_latency(&self, id: &str) -> ReadLatency {
        match self.setting_facts(id) {
            Ok((backend, ..)) if backend.is_mock() => ReadLatency::Immediate,
            Ok(_) => ReadLatency::Background,
            Err(_) => ReadLatency::Immediate,
        }
    }

    fn read(&self, id: &str) -> Result<SettingValue, LicomanError> {
        self.read_detailed(id).map(|result| result.value)
    }

    fn read_detailed(&self, id: &str) -> Result<ReadResult, LicomanError> {
        let (backend, kind, default, nullable, policy, auth) = self.setting_facts(id)?;

        if backend.is_mock() {
            let value = self.read_mock(id, kind, default, nullable)?;
            let writable = match (&policy, &auth) {
                (ApplyPolicy::ReadOnly, _) => Writable::No {
                    reason: "This setting is informational; it cannot be changed.".into(),
                },
                (_, Some(auth)) => Writable::NeedsPermission { auth: auth.clone() },
                _ => Writable::Yes,
            };
            return Ok(ReadResult {
                writable,
                value,
                origin: ValueOrigin::Default,
                freshness: crate::read::Freshness::now(crate::read::fnv1a_fingerprint(
                    id.as_bytes(),
                )),
                restart_effect: crate::read::RestartEffect::None,
                details: SourceDetails {
                    backend_kind: "mock",
                    target: format!("declared default for {id}"),
                    origin_note: Some("module-declared default (mock source)".to_string()),
                    imported: false,
                    diagnostics: Vec::new(),
                },
            });
        }

        // Real backend: bounded I/O through the router. Callers must respect
        // [`Provider::read_latency`] (Background) — the store schedules
        // these off the UI thread.
        let descriptor = {
            let registry = self.registry.lock().expect("registry lock");
            registry
                .descriptors()
                .iter()
                .find(|d| d.id == id)
                .cloned()
                .ok_or_else(|| self.setting_error(id, "This setting is not registered.".into()))?
        };
        let mut result = self.router.read(&backend, &descriptor)?;
        // Engine-backed families present as writable when the engine can
        // actually write them here (D49). Per family:
        // - file: the version/environment gate (Alacritty options, D50);
        // - dbus: the read succeeded, so the service is present;
        // - gsettings: only with the desktop gate open (D51).
        let engine = self.engine.as_ref();
        let engine_backed = match (&engine, &backend) {
            (Some(_), BackendBinding::File { .. }) => matches!(
                self.router.write_support(&backend),
                crate::read::SupportState::Available
            ),
            (Some(_), BackendBinding::DBus { .. }) => true,
            (Some(_), BackendBinding::GSettings { .. }) => self
                .gsettings_gate
                .as_ref()
                .is_some_and(|gate| gate.get() == Some(true)),
            _ => false,
        };
        if engine_backed {
            result.writable = Writable::Yes;
        }
        Ok(result)
    }

    fn write_risk(&self, id: &str) -> crate::provider::RiskLevel {
        let registry = self.registry.lock().expect("registry lock");
        registry
            .setting(id)
            .map(|(_def, setting)| setting.risk)
            .unwrap_or(crate::provider::RiskLevel::Low)
    }

    fn write_intent(
        &self,
        intent: &crate::write::ChangeIntent,
    ) -> Result<crate::provider::WriteOutcome, LicomanError> {
        let id = &intent.setting_id;
        let (backend, risk) = {
            let registry = self.registry.lock().expect("registry lock");
            registry
                .setting_status(id)
                .map_err(|reason| self.setting_error(id, reason))?;
            let (_def, setting) = registry
                .setting(id)
                .ok_or_else(|| self.setting_error(id, "This setting is not registered.".into()))?;
            (setting.backend.clone(), setting.risk)
        };
        if backend.is_mock() {
            // Mock: legacy in-memory write; the verification readback gives
            // the store its confirmation.
            let value = intent.change.value().ok_or_else(|| {
                LicomanError::new(
                    ErrorKind::Validation,
                    "A mock value cannot be removed; reset writes the declared default.",
                    format!("RemoveOverride on mock binding for {id}"),
                )
            })?;
            Provider::write(self, id, value)?;
            let verification = self.read_detailed(id)?;
            return Ok(crate::provider::WriteOutcome {
                outcome: crate::write::ApplyOutcome::mock_unverified(),
                verification: Some(verification),
                verification_error: None,
            });
        }
        let Some(engine) = &self.engine else {
            // The hard boundary, unchanged from Phase 05: no manifest-
            // declared backend is ever written without the fixture engine.
            return Err(LicomanError::new(
                ErrorKind::WriteFailed,
                format!(
                    "Licoman does not write {} backends in this build. Writes are \
                     gated behind the safe write engine (Phase 06). Nothing was \
                     modified.",
                    backend.kind_name()
                ),
                format!("write refused for non-mock binding on {id:?}"),
            ));
        };
        if matches!(backend, BackendBinding::Xfconf { .. }) {
            // Explicitly read-only backend (Phase 08): the refusal happens
            // before any request is built, and the engine refuses the target
            // again (defense in depth).
            return Err(LicomanError::new(
                ErrorKind::WriteFailed,
                "Licoman reads Xfconf channels but cannot change them yet. Writes are \
                 not enabled in this build. Nothing was modified.",
                format!("write refused for read-only xfconf binding on {id:?}"),
            ));
        }
        let descriptor = {
            let registry = self.registry.lock().expect("registry lock");
            registry
                .descriptors()
                .iter()
                .find(|d| d.id == *id)
                .cloned()
                .ok_or_else(|| self.setting_error(id, "This setting is not registered.".into()))?
        };
        let request = crate::write::ChangeRequest::resolve(&descriptor, &backend, intent, risk);
        let outcome = engine.apply(&request, &descriptor)?;
        // Fresh verification read: the store publishes this as the row's new
        // provenance (origin, fingerprint) so the next change re-checks the
        // current revision. GSettings verifies through the same native
        // transport the write used (like service targets): the dconf
        // user-db file can lag behind dconf-service, and a lagging file must
        // never turn an engine-verified write into a false conflict. The
        // next background read pass re-publishes the GVDB-based provenance.
        let (verification, verification_error) = if matches!(
            (&backend, engine.gsettings()),
            (BackendBinding::GSettings { .. }, Some(_))
        ) {
            match (&backend, engine.gsettings()) {
                (BackendBinding::GSettings { schema, key }, Some(gsettings)) => match gsettings
                    .native_read(schema, key)
                    .map_err(|error| error.to_licoman("verifying after the write"))
                {
                    Ok((value, user)) => (
                        Some(crate::read::ReadResult {
                            value,
                            origin: if user.is_some() {
                                crate::read::ValueOrigin::User
                            } else {
                                crate::read::ValueOrigin::Default
                            },
                            writable: Writable::Yes,
                            freshness: crate::read::Freshness::now(crate::read::fnv1a_fingerprint(
                                format!("native:{schema}/{key}").as_bytes(),
                            )),
                            restart_effect: crate::read::RestartEffect::Session,
                            details: crate::read::SourceDetails {
                                backend_kind: "gsettings",
                                target: format!("{schema} / {key}"),
                                origin_note: Some(
                                    "verified through the native GSettings API".to_string(),
                                ),
                                imported: false,
                                diagnostics: Vec::new(),
                            },
                        }),
                        None,
                    ),
                    Err(error) => (None, Some(error)),
                },
                _ => unreachable!("matched above"),
            }
        } else {
            match self.read_detailed(id) {
                Ok(result) => (Some(result), None),
                Err(error) => (None, Some(error)),
            }
        };
        Ok(crate::provider::WriteOutcome {
            outcome,
            verification,
            verification_error,
        })
    }

    fn reset_support(&self, id: &str) -> Option<crate::write::ResetSupport> {
        use crate::write::{Change, ResetDisclosure, ResetSupport};
        let (backend, default, kind) = {
            let registry = self.registry.lock().expect("registry lock");
            let (_def, setting) = registry.setting(id)?;
            (
                setting.backend.clone(),
                setting.default.clone(),
                setting.kind,
            )
        };
        let declared_default = default.and_then(|value| {
            let kind = kind?;
            super::registry::manifest_value_to_setting_value(&value, kind)
        });
        match (&backend, &self.engine) {
            (BackendBinding::File { .. }, Some(_engine)) => {
                let key = match &backend {
                    BackendBinding::File { key: Some(key), .. } => key.clone(),
                    _ => return None,
                };
                Some(ResetSupport {
                    disclosure: ResetDisclosure::DeletesOverride { target: key },
                    default_value: None,
                    kind: Change::RemoveOverride,
                })
            }
            (BackendBinding::GSettings { schema, key }, Some(_engine)) => {
                // GSettings reset uses the platform's actual reset semantics
                // (g_settings_reset — the override is removed, D51).
                Some(ResetSupport {
                    disclosure: ResetDisclosure::DeletesOverride {
                        target: format!("{schema}/{key}"),
                    },
                    default_value: None,
                    kind: Change::RemoveOverride,
                })
            }
            (BackendBinding::DBus { .. }, Some(_engine)) => Some(ResetSupport {
                disclosure: match &declared_default {
                    Some(value) => ResetDisclosure::WritesLiteral {
                        value: value.display(),
                    },
                    None => ResetDisclosure::NotPossible {
                        reason: "the module declares no default for this service setting"
                            .to_string(),
                    },
                },
                default_value: declared_default.clone(),
                kind: Change::Set(declared_default?),
            }),
            (BackendBinding::Mock, _) => Some(ResetSupport {
                disclosure: match &declared_default {
                    Some(value) => ResetDisclosure::WritesLiteral {
                        value: value.display(),
                    },
                    None => ResetDisclosure::NotPossible {
                        reason: "the module declares no default".to_string(),
                    },
                },
                default_value: declared_default.clone(),
                kind: Change::Set(declared_default?),
            }),
            _ => None,
        }
    }

    fn write(&self, id: &str, value: &SettingValue) -> Result<(), LicomanError> {
        let (policy, required_auth, backend) = {
            let registry = self.registry.lock().expect("registry lock");
            registry
                .setting_status(id)
                .map_err(|reason| self.setting_error(id, reason))?;
            let (_def, setting) = registry
                .setting(id)
                .ok_or_else(|| self.setting_error(id, "This setting is not registered.".into()))?;
            (
                setting.policy,
                setting.required_auth.clone(),
                setting.backend.clone(),
            )
        };

        if MOCK_FAIL_WRITES.contains(&id) {
            return Err(LicomanError::new(
                ErrorKind::WriteFailed,
                "The mock provider rejected this write.",
                format!("mock write failure showcase for {id:?}"),
            ));
        }
        if policy == ApplyPolicy::ReadOnly {
            return Err(LicomanError::new(
                ErrorKind::WriteFailed,
                "This setting is read-only.",
                format!("policy read_only for {id:?}"),
            ));
        }
        if let Some(auth) = required_auth {
            return Err(LicomanError::new(
                ErrorKind::Permission,
                format!(
                    "Requires authorization — {auth}. Licoman runs unprivileged in this build."
                ),
                format!("required_auth for {id:?}"),
            ));
        }
        if !backend.is_mock() {
            // The hard boundary: no manifest-declared backend is ever
            // written by this build.
            return Err(LicomanError::new(
                ErrorKind::WriteFailed,
                format!(
                    "Licoman does not write {} backends in this build. Writes are gated behind \
                     the safe write engine (Phase 06). Nothing was modified.",
                    backend.kind_name()
                ),
                format!("write refused for non-mock binding on {id:?}"),
            ));
        }
        self.written
            .lock()
            .expect("written lock")
            .insert(id.to_string(), value.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOCK_MODULE: &str = r#"
schema_version = 1
id = "shared.network"
scope = "shared"
title = "Mocky"
category = "Test"

[[settings]]
id = "toggle"
title = "Toggle"
type = "boolean"
control = "switch"
default = true

[settings.backend]
kind = "mock"

[[settings]]
id = "dns"
title = "DNS"
description = "Mock writes always fail to exercise the failure flow."
type = "string"
control = "text"
default = ""

[settings.backend]
kind = "mock"

[[settings]]
id = "ro"
title = "Read only"
type = "string"
control = "read_only"
policy = "read_only"
default = "informational"

[settings.backend]
kind = "mock"

[[settings]]
id = "gated"
title = "Gated"
type = "boolean"
control = "switch"
default = false
required_auth = "org.example.Admin"

[settings.backend]
kind = "mock"

[[settings]]
id = "real"
title = "Real"
type = "boolean"
control = "switch"
default = true

[settings.backend]
kind = "gsettings"
schema = "org.gnome.desktop.interface"
key = "clock-show-weekday"
"#;

    /// A dbus client stub for the fixture router (no PowerProfiles service
    /// in these tests; the dbus path is covered by the backend's own suite).
    struct AbsentService;

    impl crate::read::dbus::PowerProfilesClient for AbsentService {
        fn locate(
            &self,
        ) -> Result<
            Option<(&'static str, &'static str, &'static str)>,
            crate::read::dbus::ClientError,
        > {
            Ok(None)
        }
        fn property(
            &self,
            _endpoint: (&'static str, &'static str, &'static str),
            _name: &str,
        ) -> Result<crate::read::dbus::PropertyValue, crate::read::dbus::ClientError> {
            Err(crate::read::dbus::ClientError::Other("absent".into()))
        }
    }

    fn provider() -> ModuleProvider {
        let registry = Arc::new(Mutex::new(ModuleRegistry::load([(
            super::super::registry::ModuleSource::Builtin("m"),
            MOCK_MODULE.to_string(),
        )])));
        // Real-backend reads go through a fixture router: the synthetic
        // schema fixture plus the dconf fixtures — never the host's real
        // environment (tests must not depend on installed schemas).
        let fixtures =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
        let router = BackendRouter::new(
            crate::read::gsettings::GSettingsReadBackend::for_paths(
                vec![fixtures.join("discovery/schemas")],
                Some(fixtures.join("gsettings/dconf/user")),
                vec![fixtures.join("gsettings/dconf/system/site")],
            ),
            crate::read::file::FileReadBackend::for_dirs(
                fixtures.join("file-config"),
                fixtures.join("file-home"),
                vec![],
            ),
            crate::read::dbus::DBusReadBackend::with_client(Arc::new(AbsentService)),
            crate::read::xfconf::XfconfReadBackend::with_client(Arc::new(
                crate::read::xfconf::AbsentXfconfClient,
            )),
        );
        let provider = ModuleProvider::with_router(registry, router);
        // The Arch/Hyprland fixture carries the synthetic schema facts
        // (org.gnome.desktop.interface), so the gsettings binding passes its
        // presence check and the read path reaches the real backend.
        provider.set_environment(Some(
            &crate::discovery::demo_fixtures::arch_hyprland_wayland(),
        ));
        provider
    }

    #[test]
    fn mock_reads_serve_declared_defaults_then_written_values() {
        let provider = provider();
        assert_eq!(
            provider.read("shared.network.toggle").unwrap(),
            SettingValue::Bool(true)
        );
        provider
            .write("shared.network.toggle", &SettingValue::Bool(false))
            .unwrap();
        assert_eq!(
            provider.read("shared.network.toggle").unwrap(),
            SettingValue::Bool(false)
        );
    }

    #[test]
    fn mock_reads_carry_the_default_origin() {
        let provider = provider();
        let result = provider.read_detailed("shared.network.toggle").unwrap();
        assert_eq!(result.origin, ValueOrigin::Default);
        assert_eq!(result.writable, Writable::Yes);
        assert_eq!(result.details.backend_kind, "mock");
    }

    #[test]
    fn fail_write_showcase_id_is_rejected() {
        let provider = provider();
        let error = provider
            .write("shared.network.dns", &SettingValue::Text("x".into()))
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::WriteFailed);
    }

    #[test]
    fn read_only_and_authorized_settings_refuse_writes_with_distinct_kinds() {
        let provider = provider();
        assert_eq!(
            provider
                .write("shared.network.ro", &SettingValue::Text("x".into()))
                .unwrap_err()
                .kind,
            ErrorKind::WriteFailed
        );
        assert_eq!(
            provider
                .write("shared.network.gated", &SettingValue::Bool(true))
                .unwrap_err()
                .kind,
            ErrorKind::Permission
        );
    }

    #[test]
    fn real_backend_reads_go_through_the_router_and_writes_fail_closed() {
        let provider = provider();
        // The fixture dconf user db overrides clock-show-weekday to true —
        // a real read with real origin metadata.
        let result = provider.read_detailed("shared.network.real").unwrap();
        assert_eq!(result.details.backend_kind, "gsettings");
        assert_eq!(result.value, SettingValue::Bool(true));
        assert_eq!(result.origin, ValueOrigin::User);
        assert!(matches!(result.writable, Writable::No { .. }));
        let write_error = provider
            .write("shared.network.real", &SettingValue::Bool(false))
            .unwrap_err();
        assert_eq!(write_error.kind, ErrorKind::WriteFailed);
        assert!(write_error.user_message.contains("Nothing was modified"));
    }

    #[test]
    fn latency_reflects_the_binding_kind() {
        let provider = provider();
        assert_eq!(
            provider.read_latency("shared.network.toggle"),
            ReadLatency::Immediate
        );
        assert_eq!(
            provider.read_latency("shared.network.real"),
            ReadLatency::Background
        );
    }

    #[test]
    fn pending_environment_is_its_own_state() {
        let registry = Arc::new(Mutex::new(ModuleRegistry::load([(
            super::super::registry::ModuleSource::Builtin("m"),
            MOCK_MODULE.to_string(),
        )])));
        let provider = ModuleProvider::new(registry);
        let error = provider.read("shared.network.toggle").unwrap_err();
        assert!(error.user_message.contains("discovery has not finished"));
    }
}
