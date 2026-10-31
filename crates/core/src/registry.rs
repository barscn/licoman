//! The registry: owns providers, indexes every setting by its stable ID, and
//! routes reads/writes to the owning provider. The registry knows nothing
//! about phases or dirty tracking — that is the store's job — and nothing
//! about windows or widgets.

use std::collections::HashMap;

use crate::descriptor::SettingDescriptor;
use crate::error::{ErrorKind, LicomanError};
use crate::provider::{Capabilities, Provider};
use crate::value::SettingValue;

pub struct Registry {
    providers: Vec<Box<dyn Provider>>,
    /// id -> (provider index, descriptor)
    index: HashMap<String, (usize, SettingDescriptor)>,
    /// Descriptors in registration order (display order).
    ordered: Vec<SettingDescriptor>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            index: HashMap::new(),
            ordered: Vec::new(),
        }
    }

    /// Register a provider and index its descriptors. Duplicate setting IDs
    /// are a programmer error and fail loudly in debug builds.
    pub fn register(&mut self, provider: Box<dyn Provider>) {
        let provider_index = self.providers.len();
        let probe = provider.probe();
        for descriptor in provider.descriptors() {
            if cfg!(debug_assertions) && self.index.contains_key(descriptor.id.as_str()) {
                panic!("duplicate setting id in registry: {}", descriptor.id);
            }
            self.ordered.push(descriptor.clone());
            self.index
                .insert(descriptor.id.to_string(), (provider_index, descriptor));
        }
        let _ = probe; // probed per-read below; kept here to fail fast on panics
        self.providers.push(provider);
    }

    pub fn providers(&self) -> &[Box<dyn Provider>] {
        &self.providers
    }

    /// All descriptors in registration (display) order.
    pub fn descriptors(&self) -> &[SettingDescriptor] {
        &self.ordered
    }

    pub fn descriptor(&self, id: &str) -> Option<&SettingDescriptor> {
        self.index.get(id).map(|(_, d)| d)
    }

    pub fn capabilities(&self, id: &str) -> Option<Capabilities> {
        let (provider_index, _) = self.index.get(id)?;
        let probe = self.providers[*provider_index].probe();
        Some(probe.capabilities)
    }

    /// Route a read to the owning provider.
    pub fn read(&self, id: &str) -> Result<SettingValue, LicomanError> {
        let (provider_index, _) = self.index.get(id).ok_or_else(|| unknown_setting(id))?;
        self.providers[*provider_index].read(id)
    }

    /// Route a provenance-carrying read to the owning provider.
    pub fn read_detailed(&self, id: &str) -> Result<crate::read::ReadResult, LicomanError> {
        let (provider_index, _) = self.index.get(id).ok_or_else(|| unknown_setting(id))?;
        self.providers[*provider_index].read_detailed(id)
    }

    /// Whether this setting reads as Immediate (mock) or Background (I/O).
    pub fn read_latency(&self, id: &str) -> crate::provider::ReadLatency {
        match self.index.get(id) {
            Some((provider_index, _)) => self.providers[*provider_index].read_latency(id),
            None => crate::provider::ReadLatency::Immediate,
        }
    }

    /// Route a validated write to the owning provider.
    pub fn write(&self, id: &str, value: &SettingValue) -> Result<(), LicomanError> {
        let (provider_index, _) = self.index.get(id).ok_or_else(|| unknown_setting(id))?;
        self.providers[*provider_index].write(id, value)
    }

    /// Descriptors filtered to one scope, in registration order.
    pub fn descriptors_in_scope(&self, scope: crate::descriptor::Scope) -> Vec<&SettingDescriptor> {
        self.ordered.iter().filter(|d| d.scope == scope).collect()
    }
}

fn unknown_setting(id: &str) -> LicomanError {
    LicomanError::new(
        ErrorKind::Unavailable,
        "This setting is not registered.",
        format!("registry lookup failed for id {id:?}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{ApplyMode, Control, Scope};
    use crate::value::SettingType;

    struct OneSetting {
        value: SettingValue,
    }

    impl Provider for OneSetting {
        fn id(&self) -> &'static str {
            "one"
        }
        fn probe(&self) -> crate::provider::ProbeResult {
            crate::provider::ProbeResult::available(Capabilities {
                read: true,
                write: true,
                watch: false,
            })
        }
        fn descriptors(&self) -> Vec<SettingDescriptor> {
            vec![SettingDescriptor {
                id: "one.setting".to_string(),
                scope: Scope::Shared,
                category: "C".to_string(),
                label: "One".to_string(),
                description: String::new(),
                setting_type: SettingType::Bool,
                control: Control::Switch,
                apply_mode: ApplyMode::Staged,
                restart_target: None,
                permission_scope: None,
                note: None,
                min: None,
                max: None,
                keywords: Vec::new(),
                module_id: String::new(),
            }]
        }
        fn read(&self, _id: &str) -> Result<SettingValue, LicomanError> {
            Ok(self.value.clone())
        }
        fn write(&self, _id: &str, _value: &SettingValue) -> Result<(), LicomanError> {
            Ok(())
        }
    }

    #[test]
    fn routes_reads_and_writes_by_stable_id() {
        let mut registry = Registry::new();
        registry.register(Box::new(OneSetting {
            value: SettingValue::Bool(false),
        }));
        assert_eq!(
            registry.read("one.setting").unwrap(),
            SettingValue::Bool(false)
        );
        registry
            .write("one.setting", &SettingValue::Bool(true))
            .unwrap();
        assert!(registry.descriptor("one.setting").is_some());
        assert!(registry.read("missing").is_err());
    }

    #[test]
    fn empty_registry_has_no_settings_and_reports_unknown_ids() {
        let registry = Registry::new();
        assert!(registry.descriptors().is_empty());
        let err = registry.read("anything").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Unavailable);
    }
}
