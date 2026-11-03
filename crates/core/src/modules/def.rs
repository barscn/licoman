//! Canonical module-definition types (04_MODULE_SCHEMA_AND_REGISTRY.md §1).
//!
//! These types are the single source of truth for the module file format:
//! they deserialize TOML manifests, they serialize to the published JSON
//! Schema (via `schemars`, see `licoman module schema`), and the registry
//! converts them into descriptors. They are **data only** — nothing here can
//! execute, and validation rejects anything that looks like an execution
//! target (see [`crate::modules::manifest`]).
//!
//! Unknown fields are rejected on purpose: a typo'd field must fail loudly
//! with a source location instead of being silently ignored.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The module file format version this build understands. Distinct from any
/// application or module content version
/// (04_MODULE_SCHEMA_AND_REGISTRY.md §1).
pub const MODULE_SCHEMA_VERSION: u32 = 1;

/// Upper bound for one manifest file (DoS guard for future external sources;
/// built-in manifests are far below it).
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

/// Upper bound for settings per module.
pub const MAX_SETTINGS_PER_MODULE: usize = 512;

/// The four information-architecture scopes a module can target
/// (00_START_HERE.md). `application` modules surface in the Applications
/// window only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleScope {
    Shared,
    Desktop,
    Distribution,
    Application,
}

impl ModuleScope {
    /// The namespace prefix the module id must start with
    /// (04_MODULE_SCHEMA_AND_REGISTRY.md §1: namespaced, immutable ids).
    pub fn required_prefix(self) -> &'static str {
        match self {
            ModuleScope::Shared => "shared.",
            ModuleScope::Desktop => "desktop.",
            ModuleScope::Distribution => "distro.",
            ModuleScope::Application => "app.",
        }
    }

    /// The descriptor scope this module scope maps to.
    pub fn descriptor_scope(self) -> crate::descriptor::Scope {
        match self {
            ModuleScope::Shared => crate::descriptor::Scope::Shared,
            ModuleScope::Desktop => crate::descriptor::Scope::Desktop,
            ModuleScope::Distribution => crate::descriptor::Scope::Distribution,
            ModuleScope::Application => crate::descriptor::Scope::Application,
        }
    }
}

/// The declared value type of a setting. Strings are **not** a universal
/// data type (04_MODULE_SCHEMA_AND_REGISTRY.md §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    Boolean,
    Enum,
    Integer,
    Float,
    String,
    Path,
    /// Optional list of strings; empty = not set (e.g. a service-provided
    /// profile list). Phase 05: readable through the D-Bus backend.
    List,
}

impl ValueKind {
    /// The in-memory value type this maps to.
    pub fn setting_type(self) -> crate::value::SettingType {
        match self {
            ValueKind::Boolean => crate::value::SettingType::Bool,
            ValueKind::Enum => crate::value::SettingType::Choice,
            ValueKind::Integer => crate::value::SettingType::Integer,
            ValueKind::Float => crate::value::SettingType::Float,
            ValueKind::String => crate::value::SettingType::Text,
            ValueKind::Path => crate::value::SettingType::Path,
            ValueKind::List => crate::value::SettingType::List,
        }
    }
}

/// The UI control kind. Presentation is owned by Licoman's design system —
/// the control selects among approved components, never free-form layout
/// (04_MODULE_SCHEMA_AND_REGISTRY.md §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlKind {
    Switch,
    Checkbox,
    Dropdown,
    Number,
    Slider,
    Text,
    PathInput,
    /// A declarative action; explicitly inert until a trusted backend is
    /// wired (Phase 06+ gates actions). Rendered as an explanation.
    Action,
    /// Informational display-only value.
    ReadOnly,
}

impl ControlKind {
    /// Whether this control accepts user edits at all.
    pub fn is_editable(self) -> bool {
        matches!(
            self,
            ControlKind::Switch
                | ControlKind::Checkbox
                | ControlKind::Dropdown
                | ControlKind::Number
                | ControlKind::Slider
                | ControlKind::Text
                | ControlKind::PathInput
        )
    }
}

/// How a change reaches the source (04_MODULE_SCHEMA_AND_REGISTRY.md §1:
/// `immediate`, `staged` or `read_only`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyPolicy {
    /// Apply as soon as a valid edit is made.
    Immediate,
    /// Stage the edit until the explicit Apply action.
    #[default]
    Staged,
    /// The setting can never be written through Licoman in this phase.
    ReadOnly,
}

/// Risk classification for truthful UI treatment (parsed, validated and
/// indexed; `high` risk rows carry a warning status line).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    #[default]
    Low,
    Medium,
    High,
}

/// How completely a module integrates its target
/// (04_MODULE_SCHEMA_AND_REGISTRY.md §1: supported/partial/read-only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportLevel {
    #[default]
    Supported,
    Partial,
    ReadOnly,
    /// Only valid for the Applications-window merge (no module at all);
    /// a manifest may not declare it.
    Unsupported,
}

/// One enum option: the stored `value` plus an optional display `label`.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct EnumOption {
    pub value: String,
    #[serde(default)]
    pub label: Option<String>,
}

/// Declared application version bounds.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct VersionRange {
    #[serde(default)]
    pub min: Option<String>,
    #[serde(default)]
    pub max: Option<String>,
}

/// Where a setting's value lives. A **data-only allowlist**: manifests can
/// select supported backends and name targets, but can never carry shell
/// text, commands, code, or native library references
/// (04_MODULE_SCHEMA_AND_REGISTRY.md §4; 00_START_HERE.md non-negotiable 5).
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackendBinding {
    /// Deterministic in-memory mock values (declared defaults). Used by
    /// mock modules and the visual-verification catalog.
    Mock,
    /// A GSettings schema/key pair. Applicability requires the schema
    /// database probe AND the exact schema/key to be present.
    #[serde(rename = "gsettings")]
    GSettings { schema: String, key: String },
    /// A service on the session bus (well-known name check only in this
    /// phase; property reads arrive with Phase 05).
    #[serde(rename = "dbus")]
    DBus {
        service: String,
        path: String,
        interface: String,
        #[serde(default)]
        property: Option<String>,
    },
    /// A configuration file under `$XDG_CONFIG_HOME` (relative path, no
    /// `..`, no absolute paths — validation rejects anything else).
    /// `resolution` selects how the file is located and merged
    /// (Phase 05: TOML first; Alacritty's documented search paths and
    /// `general.import` merge are the first implemented strategy).
    File {
        path: String,
        format: FileFormat,
        #[serde(default)]
        key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resolution: Option<FileResolution>,
    },
    /// A property in an Xfconf channel (Phase 08, Wave A.3): the Xfce
    /// configuration service on the session bus. Data-only: a channel
    /// identifier plus a `/`-rooted property path. Explicitly read-only in
    /// this build — `probe` validates actual channel/property presence, the
    /// read envelope reports the honest read-only reason, and no write
    /// target exists (the engine refuses [`WriteTarget::ReadOnly`]).
    #[serde(rename = "xfconf")]
    Xfconf { channel: String, property: String },
}

/// How a `file` binding locates and merges its configuration. Closed
/// allowlist: unknown strategies are a validation error, never a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileResolution {
    /// One file relative to `$XDG_CONFIG_HOME`, read as-is.
    XdgFile,
    /// Alacritty: documented search-path list plus the `general.import`
    /// merge ("imports are loaded in order, skipping all missing files,
    /// with the importing file being loaded last").
    Alacritty,
    /// mpv (Phase 08): the documented single user configuration
    /// (`$XDG_CONFIG_HOME/mpv/mpv.conf`, legacy `~/.mpv/config`), parsed
    /// with mpv's own `key=value` / `[profile]` grammar; top-level options
    /// only. Writes are gated by the detected mpv version.
    Mpv,
}

impl BackendBinding {
    /// Stable kind name used in notes, diagnostics and tests.
    pub fn kind_name(&self) -> &'static str {
        match self {
            BackendBinding::Mock => "mock",
            BackendBinding::GSettings { .. } => "gsettings",
            BackendBinding::DBus { .. } => "dbus",
            BackendBinding::File { .. } => "file",
            BackendBinding::Xfconf { .. } => "xfconf",
        }
    }

    /// Whether values are served by the in-process mock source (true only
    /// for `mock`); every other binding is read/write-gated to later phases.
    pub fn is_mock(&self) -> bool {
        matches!(self, BackendBinding::Mock)
    }
}

/// File formats a `file` binding may declare (validated allowlist).
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileFormat {
    Toml,
    Json,
    Keyfile,
    /// mpv's own `key=value` / `[profile]` grammar (Phase 08) — parsed with
    /// the mpv rules, never TOML. Requires `resolution = "mpv"`.
    MpvConf,
}

/// Compatibility constraints, all optional and AND-ed together
/// (04_MODULE_SCHEMA_AND_REGISTRY.md §1: app id/DE/distro family, version
/// range, required provider capability).
#[derive(Debug, Clone, Default, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Compatibility {
    /// Desktop environments this module applies to (case-insensitive names
    /// like `GNOME`, `KDE`, `Hyprland`, `Xfce`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub desktops: Vec<String>,
    /// Distribution ids/families matched against os-release `ID`/`ID_LIKE`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub distros: Vec<String>,
    /// Desktop-file ids of the applications this module integrates
    /// (case-insensitive, `Name.desktop` or `name`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apps: Vec<String>,
    /// Backend capability requirements: capability ids from discovery
    /// (`config:alacritty`, `package-manager:pacman`) or kind aliases
    /// (`gsettings`, `dbus`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_backends: Vec<String>,
    /// Declared application version requirement. Licoman cannot detect
    /// application versions yet, so a declared range is **conservatively
    /// treated as incompatible** with the uncertainty explained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<VersionRange>,
}

/// Maintainer metadata for provenance and the Applications window.
#[derive(Debug, Clone, Default, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SupportInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintainer: Option<String>,
    /// Versions the module was tested against (display metadata).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tested: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<SupportLevelNoUnsupported>,
}

/// Newtype so manifests can declare every support level **except**
/// `unsupported` (a registered module that declares itself unsupported is a
/// contradiction and fails validation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportLevelNoUnsupported {
    Supported,
    Partial,
    ReadOnly,
}

impl From<SupportLevelNoUnsupported> for SupportLevel {
    fn from(level: SupportLevelNoUnsupported) -> Self {
        match level {
            SupportLevelNoUnsupported::Supported => SupportLevel::Supported,
            SupportLevelNoUnsupported::Partial => SupportLevel::Partial,
            SupportLevelNoUnsupported::ReadOnly => SupportLevel::ReadOnly,
        }
    }
}

/// Optional per-setting availability predicate (AND of the present fields),
/// evaluated against the same environment facts as module compatibility.
#[derive(Debug, Clone, Default, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AvailabilityPredicate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distro: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
}

/// A typed manifest value (the declared `default`). A closed enum instead of
/// a generic dynamic value so the JSON Schema stays precise and validation
/// can be field-level.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ManifestValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<ManifestValue>),
}

/// One setting definition (04_MODULE_SCHEMA_AND_REGISTRY.md §1).
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingDefinition {
    /// Module-relative stable id; the full id is `<module-id>.<id>`.
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Extra search terms (synonyms, related names).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// Declared value type. Required for every control except `action`.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ValueKind>,
    pub control: ControlKind,
    #[serde(default)]
    pub policy: ApplyPolicy,
    #[serde(default)]
    pub risk: RiskLevel,
    /// Authorization boundary (e.g. a polkit action id). Settings with an
    /// authorization requirement are permission-held in this phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_auth: Option<String>,
    /// Restart/persistence semantics: the named target must restart for the
    /// change to take effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<String>,
    /// Declared default value (typed per `kind`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<ManifestValue>,
    /// Explicit absence support: only `string`/`path` settings may be
    /// nullable; an empty value then means "not set", never `false`/`0`
    /// (04_MODULE_SCHEMA_AND_REGISTRY.md §3).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nullable: bool,
    /// Inclusive numeric bounds (integer/float only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Step for number/slider controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Maximum length for string/path values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<usize>,
    /// Enum options (enum kind only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<EnumOption>,
    /// Group/tab metadata: the Applications-window tab and search breadcrumb
    /// for this setting. Falls back to the module group, then the category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Display order within the module (stable sort key, then declaration
    /// order).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<f64>,
    /// Optional availability predicate (AND of present fields).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<AvailabilityPredicate>,
    /// Where the value lives (data-only allowlist; see [`BackendBinding`]).
    pub backend: BackendBinding,
}

/// A module definition: one validated manifest
/// (04_MODULE_SCHEMA_AND_REGISTRY.md §1/§2).
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleDefinition {
    pub schema_version: u32,
    /// Namespaced, immutable module id (e.g. `app.alacritty`,
    /// `desktop.gnome.interface`).
    pub id: String,
    pub scope: ModuleScope,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Navigation category (grouping metadata, not free-form layout).
    pub category: String,
    /// Default group/tab for settings that do not declare their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Display order among modules (stable sort key, then id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<f64>,
    /// Canonical upstream documentation. Optional; validated to be an
    /// `https://` URL. No other URL scheme is accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
    #[serde(default)]
    pub compatibility: Compatibility,
    #[serde(default)]
    pub support: SupportInfo,
    #[serde(default)]
    pub settings: Vec<SettingDefinition>,
}

impl ModuleDefinition {
    /// The effective support level of the module.
    pub fn support_level(&self) -> SupportLevel {
        self.support
            .level
            .map(SupportLevel::from)
            .unwrap_or(SupportLevel::Supported)
    }

    /// Full stable id for a module-relative setting id.
    pub fn full_setting_id(&self, setting: &SettingDefinition) -> String {
        format!("{}.{}", self.id, setting.id)
    }

    /// Effective group/tab name for a setting.
    pub fn setting_group(&self, setting: &SettingDefinition) -> String {
        setting
            .group
            .clone()
            .or_else(|| self.group.clone())
            .unwrap_or_else(|| self.category.clone())
    }
}
