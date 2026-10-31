//! The provider bridge: exposes the module registry through the existing
//! [`Provider`] trait so the Phase 02 [`crate::model::Store`], command
//! vocabulary and rendering paths keep working unchanged
//! (04_MODULE_SCHEMA_AND_REGISTRY.md §4: "Bind typed value state (mock reads
//! only in this phase)").
//!
//! Read/write rules, fail-closed:
//!
//! - **mock** bindings serve declared defaults (never invented values) and
//!   accept in-memory writes so staged/immediate flows remain exercisable.
//! - **gsettings / dbus / file** bindings are *declared, validated and
//!   recorded* (provenance) but reads fail with "not readable yet" and
//!   writes are refused unconditionally — no real setting is touched before
//!   the read-only backends (Phase 05) and the safe write engine (Phase 06).
//! - `read_only` policy and `required_auth` settings refuse writes with
//!   distinct, user-safe errors.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::def::ApplyPolicy;
use super::registry::ModuleRegistry;
use crate::descriptor::SettingDescriptor;
use crate::error::{ErrorKind, LicomanError};
use crate::modules::def::ValueKind;
use crate::provider::{Capabilities, ProbeResult, Provider};
use crate::value::SettingValue;

/// Mock-backed settings whose writes are rejected to exercise the visible
/// failure flow (same deterministic behavior the Phase 02 fixture catalog
/// had). Purely an in-memory mock concern; real backends refuse anyway.
const MOCK_FAIL_WRITES: &[&str] = &["shared.network.dns"];

/// Cheap to clone: the adapter holds one handle (to refresh the environment
/// and read module metadata) while another clone is registered as the
/// `Provider` behind the core `Registry`.
#[derive(Clone)]
pub struct ModuleProvider {
    registry: Arc<Mutex<ModuleRegistry>>,
    written: Arc<Mutex<HashMap<String, SettingValue>>>,
}

impl ModuleProvider {
    /// Wrap a registry. The adapter keeps a clone to update the environment;
    /// another clone stays behind the `Provider` trait.
    pub fn new(registry: Arc<Mutex<ModuleRegistry>>) -> Self {
        Self {
            registry,
            written: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Re-evaluate applicability against a snapshot. Returns whether the set
    /// of available settings changed (the adapter reloads only then).
    pub fn set_environment(
        &self,
        snapshot: Option<&crate::discovery::EnvironmentSnapshot>,
    ) -> bool {
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

    fn setting_error(&self, id: &str, reason: String) -> LicomanError {
        LicomanError::new(
            ErrorKind::Unavailable,
            reason,
            format!("module registry status for {id:?}"),
        )
    }
}

impl Provider for ModuleProvider {
    fn id(&self) -> &'static str {
        "module-registry"
    }

    fn probe(&self) -> ProbeResult {
        // Read is real (mock value source); write is advertised because
        // mock-backed settings accept in-memory writes — per-setting gating
        // happens in [`Provider::write`] and fails closed for everything
        // else.
        ProbeResult::available(Capabilities {
            read: true,
            write: true,
            watch: false,
        })
    }

    fn descriptors(&self) -> Vec<SettingDescriptor> {
        self.with_registry(|registry| registry.descriptors().to_vec())
    }

    fn read(&self, id: &str) -> Result<SettingValue, LicomanError> {
        let (value_kind, default, backend, nullable) = {
            let registry = self.registry.lock().expect("registry lock");
            registry
                .setting_status(id)
                .map_err(|reason| self.setting_error(id, reason))?;
            let (_def, setting) = registry
                .setting(id)
                .ok_or_else(|| self.setting_error(id, "This setting is not registered.".into()))?;
            (
                setting.kind,
                setting.default.clone(),
                setting.backend.clone(),
                setting.nullable,
            )
        };

        // Mock-only reads in this phase (declared defaults or explicit
        // not-set; the registry never invents a value).
        if backend.is_mock() {
            if let Some(written) = self.written.lock().expect("written lock").get(id) {
                return Ok(written.clone());
            }
            if let Some(default) = &default {
                let kind = value_kind.expect("validated: non-action settings declare a type");
                return manifest_value(default, kind).ok_or_else(|| {
                    LicomanError::new(
                        ErrorKind::Unavailable,
                        "The declared default value does not match this setting's type.",
                        format!("default coercion failed for {id:?}"),
                    )
                });
            }
            if nullable {
                return Ok(match value_kind {
                    Some(ValueKind::Path) => SettingValue::Path(String::new()),
                    _ => SettingValue::Text(String::new()),
                });
            }
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "This setting has no value yet: no default is declared and mock reads have \
                 nothing to serve. Real reads arrive with Phase 05.",
                format!("mock binding for {id:?} has no default"),
            ));
        }

        Err(LicomanError::new(
            ErrorKind::Unavailable,
            format!(
                "Not readable yet — reading {} backends arrives with real read support \
                 (Phase 05). The binding is validated and recorded.",
                backend.kind_name()
            ),
            format!("read refused for non-mock binding on {id:?}"),
        ))
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

fn manifest_value(value: &super::def::ManifestValue, kind: ValueKind) -> Option<SettingValue> {
    super::registry::manifest_value_to_setting_value(value, kind)
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

    fn provider() -> ModuleProvider {
        let registry = Arc::new(Mutex::new(ModuleRegistry::load([(
            super::super::registry::ModuleSource::Builtin("m"),
            MOCK_MODULE.to_string(),
        )])));
        let provider = ModuleProvider::new(registry);
        // The Arch/Hyprland fixture carries the synthetic schema facts
        // (org.gnome.desktop.interface), so the gsettings binding passes its
        // presence check and the read path reaches the truthful
        // "not readable yet" refusal.
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
    fn real_backend_reads_and_writes_fail_closed() {
        let provider = provider();
        let read_error = provider.read("shared.network.real").unwrap_err();
        assert_eq!(read_error.kind, ErrorKind::Unavailable);
        assert!(read_error.user_message.contains("Not readable yet"));
        let write_error = provider
            .write("shared.network.real", &SettingValue::Bool(false))
            .unwrap_err();
        assert_eq!(write_error.kind, ErrorKind::WriteFailed);
        assert!(write_error.user_message.contains("Nothing was modified"));
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
