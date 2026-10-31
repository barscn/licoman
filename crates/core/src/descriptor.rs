//! Read-only setting descriptors: identity, scope, presentation hints, and
//! metadata (apply mode, restart requirement, permission scope). Descriptors
//! describe *what a setting is*; [`crate::state`] describes *where it is*.
//!
//! Since Phase 04 descriptors are **runtime data** converted from validated
//! module definitions (04_MODULE_SCHEMA_AND_REGISTRY.md), so every text field
//! is owned. Provenance lives on the descriptor: `module_id` records which
//! module declared the setting, `keywords` feed the search index.

/// Licoman's information architecture (00_START_HERE.md). `Application`
/// settings render in the dedicated Applications window, never in the
/// main-window sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    Shared,
    Desktop,
    Distribution,
    Application,
}

impl Scope {
    pub const ALL: [Scope; 4] = [
        Scope::Shared,
        Scope::Desktop,
        Scope::Distribution,
        Scope::Application,
    ];

    /// The scopes offered in the main-window sidebar. `Application` is
    /// deliberately absent: it has its own window (00_START_HERE.md IA).
    pub const SIDEBAR: [Scope; 3] = [Scope::Shared, Scope::Desktop, Scope::Distribution];

    pub fn label(self) -> &'static str {
        match self {
            Scope::Shared => "Shared",
            Scope::Desktop => "Desktop",
            Scope::Distribution => "Distribution",
            Scope::Application => "Applications",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Scope::Shared => "Common settings on this Linux system.",
            Scope::Desktop => "Settings provided by your desktop environment.",
            Scope::Distribution => "Distribution administration and package tooling.",
            Scope::Application => "Settings for your installed applications.",
        }
    }
}

/// How a change reaches the source: immediately on edit, or staged until an
/// explicit Apply. This is **metadata on the descriptor** — the UI never
/// decides apply semantics per widget (02_FOUNDATION.md §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    /// Apply as soon as a valid edit is made (theme-like flows).
    Immediate,
    /// Stage the edit; apply only through the explicit Apply action.
    Staged,
}

/// The control the UI should render. Pure presentation hint — behavior still
/// flows through commands and phases. `Action` is explicitly inert until a
/// trusted backend is wired (04_MODULE_SCHEMA_AND_REGISTRY.md §3).
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    Switch,
    /// Dropdown with a fixed option list.
    Choice {
        options: Vec<String>,
    },
    Slider {
        step: f64,
        unit: String,
    },
    Number {
        step: f64,
        unit: String,
    },
    Text {
        placeholder: String,
    },
    Path,
    List,
    /// A declarative action. Inert: rendered as an explanation, never as a
    /// button that pretends to work without a trusted backend.
    Action,
    /// Display-only value (read-only, unavailable, or permission-held rows).
    ReadOnly,
}

/// Stable identity and metadata for one setting. `id` is permanent: search
/// results, models and providers key on it (02_FOUNDATION.md tests §2).
/// For manifest settings the id is the full namespaced form
/// `<module-id>.<setting-id>` (04_MODULE_SCHEMA_AND_REGISTRY.md §1).
#[derive(Debug, Clone, PartialEq)]
pub struct SettingDescriptor {
    pub id: String,
    pub scope: Scope,
    pub category: String,
    pub label: String,
    pub description: String,
    pub setting_type: crate::value::SettingType,
    pub control: Control,
    pub apply_mode: ApplyMode,
    /// Known restart requirement: applying this setting takes effect after
    /// restarting the named target. Discovered-at-write-time requirements are
    /// recorded on the model instead.
    pub restart_target: Option<String>,
    /// When present, the setting is held behind an authorization boundary
    /// Licoman does not cross in this phase.
    pub permission_scope: Option<String>,
    /// Provider-supplied status hint rendered as the row's status line (e.g.
    /// why a setting is read-only here). Not an error — errors live on the
    /// model.
    pub note: Option<String>,
    /// Numeric bounds for Integer/Float values.
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Extra search terms from the module definition (synonyms, app names).
    pub keywords: Vec<String>,
    /// Provenance: the module that declared this setting (empty for
    /// hand-built test descriptors).
    pub module_id: String,
}

impl SettingDescriptor {
    /// Descriptor with the common metadata defaulted; mutate the result for
    /// restart targets, permission scopes, notes or numeric bounds.
    pub fn new(
        id: impl Into<String>,
        scope: Scope,
        category: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
        setting_type: crate::value::SettingType,
        control: Control,
    ) -> Self {
        Self {
            id: id.into(),
            scope,
            category: category.into(),
            label: label.into(),
            description: description.into(),
            setting_type,
            control,
            apply_mode: ApplyMode::Staged,
            restart_target: None,
            permission_scope: None,
            note: None,
            min: None,
            max: None,
            keywords: Vec::new(),
            module_id: String::new(),
        }
    }

    /// Breadcrumb for search results: scope → category → setting.
    pub fn breadcrumb(&self) -> String {
        format!(
            "{} → {} → {}",
            self.scope.label(),
            self.category,
            self.label
        )
    }
}
