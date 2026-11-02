//! The module registry (04_MODULE_SCHEMA_AND_REGISTRY.md §4):
//!
//! ```text
//! Load built-in manifests
//!    -> Validate schema and unique IDs
//!    -> Query EnvironmentSnapshot and provider capabilities
//!    -> Compute applicability and compatibility reasons
//!    -> Index scopes, categories, groups, apps and search terms
//!    -> Bind typed value state (mock reads only in this phase)
//!    -> Provide normalized data to GPUI adapter
//! ```
//!
//! Source precedence (explicit, §4): **built-in manifests are embedded at
//! compile time** (`BUILTIN_MANIFESTS`) — trusted at build/release time, with
//! no runtime override channel at all. External data-only module loading is
//! deliberately deferred until a separate security review; the loader below
//! is generic over `(source, text)` pairs so the read-only linter and tests
//! can exercise it without the app ever opening untrusted paths.
//!
//! Collision policy: duplicate module ids reject the *later* definition
//! (first-loaded wins, never last-one-wins, and unrelated valid modules
//! still load); duplicate setting ids within a module reject that module.

use std::collections::HashMap;

use super::compat::{self, ApplicabilityState, EnvironmentFacts, ModuleApplicability};
use super::def::{
    ApplyPolicy, ControlKind, MODULE_SCHEMA_VERSION, ModuleDefinition, ModuleScope,
    SettingDefinition, SupportLevel, ValueKind,
};
use super::manifest::{ManifestIssue, parse_manifest, validate_definition};
use crate::descriptor::{ApplyMode, Control, Scope, SettingDescriptor};

/// Where a manifest came from (provenance; rendered in diagnostics and the
/// UI's Source details).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleSource {
    /// Embedded at compile time (`include_str!`) — trusted at build time.
    Builtin(&'static str),
    /// A path validated by the caller (linter/test use only in this phase).
    File(std::path::PathBuf),
}

impl ModuleSource {
    pub fn display(&self) -> String {
        match self {
            ModuleSource::Builtin(name) => format!("built-in:{name}"),
            ModuleSource::File(path) => path.display().to_string(),
        }
    }
}

/// A module that loaded, validated and carries its current applicability.
#[derive(Debug, Clone)]
pub struct LoadedModule {
    pub def: ModuleDefinition,
    pub source: ModuleSource,
    pub applicability: ModuleApplicability,
}

/// A module rejected at load — kept with its diagnostics so the contributor
/// (and `licoman module validate`) can see exactly why.
#[derive(Debug, Clone)]
pub struct RejectedModule {
    pub source: ModuleSource,
    pub id: Option<String>,
    pub issues: Vec<ManifestIssue>,
}

/// An application-scope module summary for the Applications-window merge.
#[derive(Debug, Clone, PartialEq)]
pub struct AppModuleRef {
    pub module_id: String,
    pub name: String,
    pub level: SupportLevel,
    /// Desktop-file ids this module integrates.
    pub apps: Vec<String>,
    /// Effective config path / schema summary (provenance for Source
    /// details): the distinct backend kinds + targets used by its settings.
    pub provenance: Vec<String>,
}

/// The registry: loaded modules, rejection diagnostics, indexes and the
/// environment facts applicability was last evaluated against.
#[derive(Debug, Default)]
pub struct ModuleRegistry {
    modules: Vec<LoadedModule>,
    rejected: Vec<RejectedModule>,
    by_module_id: HashMap<String, usize>,
    /// Full setting id → (module index, setting index).
    setting_index: HashMap<String, (usize, usize)>,
    /// Cached descriptor conversion (structural; environment-independent).
    descriptors: Vec<SettingDescriptor>,
    /// Facts applicability was last evaluated against (None = pending).
    facts: Option<EnvironmentFacts>,
    /// Flag: descriptors keep their natural (editable) controls for
    /// non-mock backends. The provider enables it only when a write engine
    /// is attached — fixture engines (demos/tests, injected roots) or the
    /// live engine (Phase 07, D49). Every actual write still goes through
    /// the engine's policy/value/target gates; this flag only governs
    /// presentation.
    write_controls: bool,
}

impl ModuleRegistry {
    /// The embedded built-in manifests, in deterministic load order.
    pub fn builtin_manifests() -> Vec<(ModuleSource, String)> {
        crate::modules::builtin::MANIFESTS
            .iter()
            .map(|(name, text)| (ModuleSource::Builtin(name), text.to_string()))
            .collect()
    }

    /// Load the built-in manifests.
    pub fn load_builtin() -> Self {
        Self::load(Self::builtin_manifests())
    }

    /// Load from explicit `(source, text)` pairs. Never panics and never
    /// loads a module that failed validation: rejections are recorded with
    /// diagnostics while unrelated valid modules still load (§4).
    pub fn load<S: IntoIterator<Item = (ModuleSource, String)>>(sources: S) -> Self {
        let mut registry = Self::default();
        for (source, text) in sources {
            registry.ingest(source, &text);
        }
        registry.finish();
        registry
    }

    fn ingest(&mut self, source: ModuleSource, text: &str) {
        let def = match parse_manifest(text) {
            Ok(def) => def,
            Err(issues) => {
                self.rejected.push(RejectedModule {
                    id: None,
                    source,
                    issues,
                });
                return;
            }
        };
        let mut issues = validate_definition(&def);
        issues.retain(|issue| issue.severity == super::manifest::IssueSeverity::Error);
        if !issues.is_empty() {
            self.rejected.push(RejectedModule {
                id: Some(def.id.clone()),
                source,
                issues,
            });
            return;
        }
        if self.by_module_id.contains_key(&def.id) {
            self.rejected.push(RejectedModule {
                id: Some(def.id.clone()),
                source,
                issues: vec![super::manifest::ManifestIssue::error(
                    super::manifest::IssueLocation::Field("id"),
                    format!(
                        "duplicate module id {id:?}: the first-loaded definition wins and this \
                         conflicting module is rejected (never last-one-wins, and a file named \
                         like a built-in module can never override it)",
                        id = def.id
                    ),
                )],
            });
            return;
        }

        // Deterministic display order: explicit `order`, then declaration
        // order (stable sort).
        let mut settings: Vec<SettingDefinition> = def.settings.clone();
        settings.sort_by(|a, b| {
            a.order
                .partial_cmp(&b.order)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut sorted = def.clone();
        sorted.settings = settings;

        self.by_module_id.insert(def.id.clone(), self.modules.len());
        self.modules.push(LoadedModule {
            def: sorted,
            source,
            applicability: ModuleApplicability::pending(),
        });
    }

    /// Sort modules (order, then id) and build the descriptors + setting
    /// index. Called once at the end of [`ModuleRegistry::load`].
    fn finish(&mut self) {
        self.modules.sort_by(|a, b| {
            let a_order = a.def.order.unwrap_or(f64::INFINITY);
            let b_order = b.def.order.unwrap_or(f64::INFINITY);
            a_order
                .partial_cmp(&b_order)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.def.id.cmp(&b.def.id))
        });
        self.by_module_id.clear();
        for (index, module) in self.modules.iter().enumerate() {
            self.by_module_id.insert(module.def.id.clone(), index);
        }

        self.descriptors.clear();
        self.setting_index.clear();
        for (module_index, module) in self.modules.iter().enumerate() {
            for (setting_index, setting) in module.def.settings.iter().enumerate() {
                let full_id = module.def.full_setting_id(setting);
                self.descriptors
                    .push(descriptor_for(module, setting, self.write_controls));
                self.setting_index
                    .insert(full_id, (module_index, setting_index));
            }
        }
    }

    /// Keep the natural (editable) controls for non-mock backends: called
    /// when a write engine is attached, fixture-backed (Phase 06 demos and
    /// tests) or live (Phase 07, D49). Returns whether descriptors were
    /// rebuilt. Presentation only — the engine re-derives and re-enforces
    /// every policy at write time.
    pub fn enable_write_controls(&mut self) -> bool {
        if self.write_controls {
            return false;
        }
        self.write_controls = true;
        self.finish();
        true
    }

    // ---- environment -------------------------------------------------------

    /// Re-evaluate applicability against a snapshot (or clear to pending).
    /// Returns whether the set of available settings changed — the adapter
    /// reloads the store only when it did, preserving navigation and staged
    /// edits otherwise (§6).
    pub fn set_environment(
        &mut self,
        snapshot: Option<&crate::discovery::EnvironmentSnapshot>,
    ) -> bool {
        let facts = snapshot.map(EnvironmentFacts::from_snapshot);
        let before: std::collections::BTreeSet<String> = self.available_ids();
        for module in &mut self.modules {
            module.applicability = compat::evaluate(&module.def, facts.as_ref());
        }
        self.facts = facts;
        let after: std::collections::BTreeSet<String> = self.available_ids();
        before != after
    }

    /// Full ids of settings that are available right now (module applicable
    /// + binding/predicate checks pass).
    fn available_ids(&self) -> std::collections::BTreeSet<String> {
        self.setting_index
            .keys()
            .filter(|id| self.setting_status(id).is_ok())
            .cloned()
            .collect()
    }

    /// Whether one setting is usable right now, with the preserved reason on
    /// failure. Pending environment is its own state, never a guess.
    pub fn setting_status(&self, id: &str) -> Result<(), String> {
        let Some((module_index, setting_index)) = self.setting_index.get(id) else {
            return Err(format!("No module declares setting {id:?}."));
        };
        let module = &self.modules[*module_index];
        match module.applicability.state {
            ApplicabilityState::Pending => {
                return Err(
                    "Environment discovery has not finished yet; availability is being \
                     determined."
                        .to_string(),
                );
            }
            ApplicabilityState::Inapplicable => {
                return Err(module.applicability.reason_text().unwrap_or_else(|| {
                    "The module is not compatible with this environment.".to_string()
                }));
            }
            ApplicabilityState::Applicable => {}
        }
        let setting = &module.def.settings[*setting_index];
        compat::setting_status(&module.def, setting, self.facts.as_ref())
    }

    // ---- accessors ---------------------------------------------------------

    pub fn modules(&self) -> &[LoadedModule] {
        &self.modules
    }

    pub fn rejected(&self) -> &[RejectedModule] {
        &self.rejected
    }

    pub fn module(&self, id: &str) -> Option<&LoadedModule> {
        self.by_module_id.get(id).map(|i| &self.modules[*i])
    }

    /// The module + setting behind a full setting id.
    pub fn setting(&self, id: &str) -> Option<(&ModuleDefinition, &SettingDefinition)> {
        let (module_index, setting_index) = self.setting_index.get(id)?;
        let module = &self.modules[*module_index];
        Some((&module.def, &module.def.settings[*setting_index]))
    }

    /// The module that owns a full setting id.
    pub fn module_of_setting(&self, id: &str) -> Option<&LoadedModule> {
        let (module_index, _) = self.setting_index.get(id)?;
        Some(&self.modules[*module_index])
    }

    /// Full setting ids of one module, in display order.
    pub fn setting_ids_of_module(&self, module_id: &str) -> Vec<String> {
        self.module(module_id)
            .map(|module| {
                module
                    .def
                    .settings
                    .iter()
                    .map(|s| module.def.full_setting_id(s))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Cached descriptor conversion in display order.
    pub fn descriptors(&self) -> &[SettingDescriptor] {
        &self.descriptors
    }

    /// Application-scope module summaries for the Applications-window merge.
    pub fn app_modules(&self) -> Vec<AppModuleRef> {
        self.modules
            .iter()
            .filter(|m| m.def.scope == ModuleScope::Application)
            .map(|m| AppModuleRef {
                module_id: m.def.id.clone(),
                name: m.def.title.clone(),
                level: m.def.support_level(),
                apps: m.def.compatibility.apps.clone(),
                provenance: provenance_lines(&m.def),
            })
            .collect()
    }

    /// Provenance lines for one module (Source details): backend bindings,
    /// docs url and tested versions.
    pub fn provenance_of(&self, module_id: &str) -> Vec<String> {
        self.module(module_id)
            .map(|m| provenance_lines(&m.def))
            .unwrap_or_default()
    }

    pub fn schema_version(&self) -> u32 {
        MODULE_SCHEMA_VERSION
    }
}

/// Provenance summary lines: distinct backend targets, docs url, tested
/// versions (04_MODULE_SCHEMA_AND_REGISTRY.md §5 — powers `Source details`).
fn provenance_lines(def: &ModuleDefinition) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for setting in &def.settings {
        let line = match &setting.backend {
            super::def::BackendBinding::Mock => continue,
            super::def::BackendBinding::GSettings { schema, key } => {
                format!("gsettings {schema} / {key}")
            }
            super::def::BackendBinding::DBus {
                service,
                interface,
                property,
                ..
            } => match property {
                Some(property) => format!("dbus {service} {interface}.{property}"),
                None => format!("dbus {service} {interface}"),
            },
            super::def::BackendBinding::File {
                path, format, key, ..
            } => match key {
                Some(key) => format!("file {} ({format:?}) key {key}", path),
                None => format!("file {} ({format:?})", path),
            },
        };
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    if let Some(url) = &def.docs_url {
        lines.push(format!("docs: {url}"));
    }
    if !def.support.tested.is_empty() {
        lines.push(format!("tested: {}", def.support.tested.join(", ")));
    }
    if let Some(maintainer) = &def.support.maintainer {
        lines.push(format!("maintainer: {maintainer}"));
    }
    lines
}

/// Convert one setting definition into the descriptor the existing store and
/// rendering paths consume. Structural only — environment-dependent
/// availability lives in [`ModuleRegistry::setting_status`].
pub fn descriptor_for(
    module: &LoadedModule,
    setting: &SettingDefinition,
    write_controls: bool,
) -> SettingDescriptor {
    let def = &module.def;
    let full_id = def.full_setting_id(setting);

    let (setting_type, control, apply_mode) = match setting.control {
        ControlKind::Action => (
            crate::value::SettingType::Text,
            Control::Action,
            ApplyMode::Staged,
        ),
        _ => {
            let kind = setting
                .kind
                .expect("validated: non-action settings declare a type");
            let control = match setting.control {
                ControlKind::Switch | ControlKind::Checkbox => Control::Switch,
                ControlKind::Dropdown => Control::Choice {
                    options: setting.options.iter().map(|o| o.value.clone()).collect(),
                },
                ControlKind::Number => Control::Number {
                    step: setting.step.unwrap_or(1.),
                    unit: setting.unit.clone().unwrap_or_default(),
                },
                ControlKind::Slider => Control::Slider {
                    step: setting.step.unwrap_or(1.),
                    unit: setting.unit.clone().unwrap_or_default(),
                },
                ControlKind::Text => Control::Text {
                    placeholder: String::new(),
                },
                ControlKind::PathInput => Control::Path,
                ControlKind::ReadOnly => Control::ReadOnly,
                ControlKind::Action => unreachable!("handled above"),
            };
            let apply_mode = match setting.policy {
                ApplyPolicy::Immediate => ApplyMode::Immediate,
                ApplyPolicy::Staged | ApplyPolicy::ReadOnly => ApplyMode::Staged,
            };
            (kind.setting_type(), control, apply_mode)
        }
    };

    // Non-mock backends render read-only unless a write engine is attached
    // (Phase 06 fixtures / Phase 07 live, D49). Every actual write still
    // passes the engine's gates.
    let (control, apply_mode) = if !setting.backend.is_mock() && !write_controls {
        (Control::ReadOnly, ApplyMode::Staged)
    } else {
        (control, apply_mode)
    };

    let mut keywords = setting.keywords.clone();
    if !keywords.iter().any(|k| k == &def.title) {
        keywords.push(def.title.clone());
    }
    for app in &def.compatibility.apps {
        let name = compat::normalize_app_id(app);
        if !keywords.contains(&name) {
            keywords.push(name);
        }
    }
    if def.scope == ModuleScope::Application {
        // Application modules are searchable by their app name: the last
        // id segment (e.g. `zed` for `app.zed`).
        let app_name = def.id.rsplit('.').next().unwrap_or_default().to_string();
        if !app_name.is_empty() && !keywords.contains(&app_name) {
            keywords.push(app_name);
        }
    }

    let mut descriptor = SettingDescriptor {
        id: full_id,
        scope: match def.scope {
            ModuleScope::Shared => Scope::Shared,
            ModuleScope::Desktop => Scope::Desktop,
            ModuleScope::Distribution => Scope::Distribution,
            ModuleScope::Application => Scope::Application,
        },
        category: def.category.clone(),
        label: setting.title.clone(),
        description: setting.description.clone(),
        setting_type,
        control,
        apply_mode,
        restart_target: setting.restart.clone(),
        permission_scope: setting.required_auth.clone(),
        note: None,
        min: setting.min,
        max: setting.max,
        keywords,
        module_id: def.id.clone(),
    };
    // Read-only presentation for declared read_only policies and the
    // real-backend note (why this row cannot be edited yet).
    if setting.policy == ApplyPolicy::ReadOnly {
        descriptor.control = Control::ReadOnly;
        if descriptor.note.is_none() {
            descriptor.note = Some("Declared read-only by the module.".to_string());
        }
    }
    if let Some(note) = compat::backend_note(&setting.backend) {
        descriptor.note = Some(note);
    }
    if setting.risk == super::def::RiskLevel::High {
        descriptor.note = Some(
            descriptor
                .note
                .map(|n| format!("High-impact setting. {n}"))
                .unwrap_or_else(|| "High-impact setting.".to_string()),
        );
    }
    descriptor
}

/// Convert a declared default into the typed value the mock source serves.
/// Coercion is explicit and kind-checked (never stringly-typed).
pub fn manifest_value_to_setting_value(
    value: &super::def::ManifestValue,
    kind: ValueKind,
) -> Option<crate::value::SettingValue> {
    use crate::value::SettingValue as SV;
    match (kind, value) {
        (ValueKind::Boolean, super::def::ManifestValue::Bool(b)) => Some(SV::Bool(*b)),
        (ValueKind::Enum, super::def::ManifestValue::Str(s)) => Some(SV::Choice(s.clone())),
        (ValueKind::Integer, super::def::ManifestValue::Int(n)) => Some(SV::Integer(*n)),
        (ValueKind::Float, super::def::ManifestValue::Float(n)) => Some(SV::Float(*n)),
        (ValueKind::Float, super::def::ManifestValue::Int(n)) => Some(SV::Float(*n as f64)),
        (ValueKind::String, super::def::ManifestValue::Str(s)) => Some(SV::Text(s.clone())),
        (ValueKind::Path, super::def::ManifestValue::Str(s)) => Some(SV::Path(s.clone())),
        (ValueKind::List, super::def::ManifestValue::List(items)) => {
            let mut strings = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    super::def::ManifestValue::Str(s) => strings.push(s.clone()),
                    _ => return None,
                }
            }
            Some(SV::List(strings))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = r#"
schema_version = 1
id = "shared.test.alpha"
scope = "shared"
title = "Alpha"
category = "Test"

[[settings]]
id = "one"
title = "One"
type = "boolean"
control = "switch"
default = true

[settings.backend]
kind = "mock"
"#;

    const B: &str = r#"
schema_version = 1
id = "desktop.test.beta"
scope = "desktop"
title = "Beta"
category = "Test"
order = 1

[[settings]]
id = "two"
title = "Two"
type = "boolean"
control = "switch"
default = false

[settings.backend]
kind = "mock"
"#;

    #[test]
    fn loads_valid_modules_and_indexes_stable_full_ids() {
        let registry = ModuleRegistry::load([
            (ModuleSource::Builtin("a"), A.to_string()),
            (ModuleSource::Builtin("b"), B.to_string()),
        ]);
        assert_eq!(registry.modules().len(), 2);
        assert!(registry.rejected().is_empty());
        let (def, setting) = registry.setting("shared.test.alpha.one").unwrap();
        assert_eq!(def.id, "shared.test.alpha");
        assert_eq!(setting.id, "one");
        assert_eq!(registry.descriptors().len(), 2);
        // B declares order = 1, so it sorts before the unordered A.
        assert_eq!(registry.descriptors()[0].id, "desktop.test.beta.two");
        assert_eq!(registry.descriptors()[1].id, "shared.test.alpha.one");
    }

    #[test]
    fn duplicate_module_id_rejects_the_later_definition_and_keeps_others() {
        let a2 = A.replace("title = \"Alpha\"", "title = \"Alpha again\"");
        let registry = ModuleRegistry::load([
            (ModuleSource::Builtin("a"), A.to_string()),
            (ModuleSource::Builtin("a2"), a2),
            (ModuleSource::Builtin("b"), B.to_string()),
        ]);
        assert_eq!(registry.modules().len(), 2, "unrelated valid module loads");
        assert_eq!(registry.rejected().len(), 1);
        assert_eq!(
            registry.rejected()[0].id.as_deref(),
            Some("shared.test.alpha")
        );
        assert!(
            registry.rejected()[0]
                .issues
                .iter()
                .any(|i| i.message.contains("duplicate module id"))
        );
        assert_eq!(
            registry.module("shared.test.alpha").unwrap().def.title,
            "Alpha"
        );
    }

    #[test]
    fn invalid_module_is_isolated_and_diagnosed() {
        let broken = A.replace("schema_version = 1", "schema_version = 7");
        let registry = ModuleRegistry::load([
            (ModuleSource::Builtin("broken"), broken),
            (ModuleSource::Builtin("b"), B.to_string()),
        ]);
        assert_eq!(registry.modules().len(), 1);
        assert_eq!(registry.rejected().len(), 1);
        assert!(
            registry.rejected()[0]
                .issues
                .iter()
                .any(|i| i.message.contains("schema version 7"))
        );
    }

    #[test]
    fn modules_and_settings_are_ordered_by_declared_order() {
        let mut b = B.to_string();
        b = b.replace("order = 1", "order = -1");
        let registry = ModuleRegistry::load([
            (ModuleSource::Builtin("a"), A.to_string()),
            (ModuleSource::Builtin("b"), b),
        ]);
        assert_eq!(registry.modules()[0].def.id, "desktop.test.beta");
    }

    #[test]
    fn setting_ids_of_module_are_display_ordered_and_prefixed() {
        let registry = ModuleRegistry::load([(ModuleSource::Builtin("a"), A.to_string())]);
        assert_eq!(
            registry.setting_ids_of_module("shared.test.alpha"),
            vec!["shared.test.alpha.one".to_string()]
        );
        assert!(registry.setting_ids_of_module("missing").is_empty());
    }

    #[test]
    fn app_modules_summarize_application_scope_only() {
        let app_module = r#"
schema_version = 1
id = "app.alacritty"
scope = "application"
title = "Alacritty"
category = "Terminal"

[compatibility]
apps = ["alacritty.desktop"]
required_backends = ["config:alacritty"]

[support]
level = "supported"

[[settings]]
id = "font_size"
title = "Font size"
group = "Appearance"
type = "float"
control = "slider"
min = 6.0
max = 32.0
default = 13.0

[settings.backend]
kind = "file"
path = "alacritty/alacritty.toml"
format = "toml"
key = "font.size"
"#;
        let registry = ModuleRegistry::load([
            (ModuleSource::Builtin("a"), A.to_string()),
            (ModuleSource::Builtin("app"), app_module.to_string()),
        ]);
        let app_modules = registry.app_modules();
        assert_eq!(app_modules.len(), 1);
        assert_eq!(app_modules[0].module_id, "app.alacritty");
        assert_eq!(app_modules[0].level, SupportLevel::Supported);
        assert_eq!(app_modules[0].apps, vec!["alacritty.desktop"]);
        assert!(
            app_modules[0]
                .provenance
                .iter()
                .any(|l| l.contains("alacritty/alacritty.toml"))
        );
    }

    #[test]
    fn setting_status_reflects_environment_transitions() {
        let mut registry = ModuleRegistry::load([(ModuleSource::Builtin("a"), A.to_string())]);
        let id = "shared.test.alpha.one";
        assert!(registry.setting_status(id).is_err(), "pending environment");
        registry.set_environment(None);
        assert!(registry.setting_status(id).is_err(), "still pending");
        // Applicable with a real snapshot (mock backend, no constraints).
        let snapshot = crate::discovery::demo_fixtures::empty_environment();
        assert!(registry.set_environment(Some(&snapshot)));
    }

    #[test]
    fn descriptor_conversion_carries_provenance_and_readonly_backends() {
        let app_module = r#"
schema_version = 1
id = "app.zed"
scope = "application"
title = "Zed"
category = "Editor"

[[settings]]
id = "theme"
title = "Theme"
keywords = ["dark"]
type = "string"
control = "read_only"
policy = "read_only"

[settings.backend]
kind = "file"
path = "zed/settings.json"
format = "json"
"#;
        let registry = ModuleRegistry::load([(ModuleSource::Builtin("z"), app_module.to_string())]);
        let descriptor = &registry.descriptors()[0];
        assert_eq!(descriptor.id, "app.zed.theme");
        assert_eq!(descriptor.scope, Scope::Application);
        assert_eq!(descriptor.control, Control::ReadOnly);
        assert_eq!(descriptor.module_id, "app.zed");
        assert!(descriptor.note.as_deref().unwrap().contains("file backend"));
        assert!(descriptor.keywords.contains(&"zed".to_string()));
        assert!(descriptor.keywords.contains(&"dark".to_string()));
    }
}
