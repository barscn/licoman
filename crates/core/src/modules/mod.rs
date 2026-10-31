//! Declarative module schema and registry (04_MODULE_SCHEMA_AND_REGISTRY.md).
//!
//! Modules are **data-only** TOML manifests that describe settings — what
//! they mean, how they are controlled, where their values live and whether
//! they apply to the detected environment. They can never execute code,
//! write host files or elevate privileges; the design system owns all
//! presentation and the registry only produces typed, validated data for
//! the existing store/rendering paths.
//!
//! Layout:
//!
//! - [`def`]: canonical Rust types (also the source of the published JSON
//!   Schema via `schemars`).
//! - [`manifest`]: TOML loading + semantic validation with located,
//!   actionable issues.
//! - [`compat`]: pure applicability evaluation over environment facts with
//!   distinct, preserved incompatibility reasons.
//! - [`registry`]: loading, indexing, provenance, application-scope
//!   summaries; built-in manifests are embedded at build time.
//! - [`provider`]: the [`crate::provider::Provider`] bridge feeding the
//!   existing [`crate::model::Store`] — mock value binding only in this
//!   phase, real backends fail closed on read and write.
//! - [`builtin`]: the embedded manifest table.

pub mod builtin;
pub mod compat;
pub mod def;
pub mod manifest;
pub mod provider;
pub mod registry;

#[cfg(test)]
#[path = "integration_tests.rs"]
mod integration_tests;

pub use compat::{
    ApplicabilityState, EnvironmentFacts, IncompatibilityDimension, IncompatibilityReason,
    ModuleApplicability,
};
pub use def::{
    ApplyPolicy, AvailabilityPredicate, BackendBinding, Compatibility, ControlKind, EnumOption,
    FileFormat, MODULE_SCHEMA_VERSION, ManifestValue, ModuleDefinition, ModuleScope, RiskLevel,
    SettingDefinition, SupportInfo, SupportLevel, ValueKind, VersionRange,
};
pub use manifest::{IssueLocation, IssueSeverity, ManifestIssue};
pub use provider::ModuleProvider;
pub use registry::{AppModuleRef, LoadedModule, ModuleRegistry, ModuleSource};

#[cfg(test)]
mod tests {
    use super::*;

    /// Every embedded built-in manifest must parse and validate cleanly —
    /// a broken built-in is a build failure, not a runtime surprise.
    #[test]
    fn builtin_manifests_are_valid() {
        let registry = ModuleRegistry::load_builtin();
        assert!(
            registry.rejected().is_empty(),
            "built-in manifests must all be valid: {:#?}",
            registry
                .rejected()
                .iter()
                .flat_map(|r| r
                    .issues
                    .iter()
                    .map(|i| i.render(r.source.display().as_str())))
                .collect::<Vec<_>>()
        );
        assert!(
            registry.modules().len() >= 10,
            "the shipped catalog expects the full built-in set"
        );
        for module in registry.modules() {
            assert_eq!(module.def.schema_version, MODULE_SCHEMA_VERSION);
            for setting in &module.def.settings {
                let full_id = module.def.full_setting_id(setting);
                // Mock-backed settings must have a servable value; every
                // other binding must be one of the allowlisted kinds.
                if setting.backend.is_mock() && setting.control != def::ControlKind::Action {
                    assert!(
                        setting.default.is_some()
                            || matches!(
                                setting.kind,
                                Some(def::ValueKind::String) | Some(def::ValueKind::Path)
                            ),
                        "{full_id}: mock binding without a servable value"
                    );
                }
            }
        }
    }

    /// The shipped catalog: 190+ settings exercising all four scopes and
    /// every control kind (04_MODULE_SCHEMA_AND_REGISTRY.md work item 5;
    /// Phase 05 rebinds some settings to real backends, lowering the
    /// mock-served count slightly).
    #[test]
    fn builtin_catalog_has_190_settings_across_all_scopes_and_controls() {
        let registry = ModuleRegistry::load_builtin();
        let descriptors = registry.descriptors();
        assert!(
            descriptors.len() >= 190,
            "expected 190+ settings, found {}",
            descriptors.len()
        );
        let mut scopes: std::collections::BTreeSet<crate::descriptor::Scope> =
            std::collections::BTreeSet::new();
        let mut controls: std::collections::BTreeSet<&'static str> =
            std::collections::BTreeSet::new();
        for descriptor in descriptors {
            scopes.insert(descriptor.scope);
            controls.insert(match descriptor.control {
                crate::descriptor::Control::Switch => "Switch",
                crate::descriptor::Control::Choice { .. } => "Choice",
                crate::descriptor::Control::Slider { .. } => "Slider",
                crate::descriptor::Control::Number { .. } => "Number",
                crate::descriptor::Control::Text { .. } => "Text",
                crate::descriptor::Control::Path => "Path",
                crate::descriptor::Control::List => "List",
                crate::descriptor::Control::Action => "Action",
                crate::descriptor::Control::ReadOnly => "ReadOnly",
            });
        }
        assert_eq!(scopes.len(), 4, "all four scopes exercised: {scopes:?}");
        for control in [
            "Switch", "Choice", "Slider", "Number", "Text", "Path", "Action", "ReadOnly",
        ] {
            assert!(
                controls.contains(control),
                "control {control} missing from the catalog: {controls:?}"
            );
        }
    }

    /// Full setting ids are globally unique across all built-ins.
    #[test]
    fn builtin_setting_ids_are_globally_unique() {
        let registry = ModuleRegistry::load_builtin();
        let ids: std::collections::BTreeSet<&str> = registry
            .descriptors()
            .iter()
            .map(|d| d.id.as_str())
            .collect();
        assert_eq!(ids.len(), registry.descriptors().len());
    }
}
