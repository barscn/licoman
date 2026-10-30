//! Read-only setting descriptors: identity, scope, presentation hints, and
//! metadata (apply mode, restart requirement, permission scope). Descriptors
//! describe *what a setting is*; [`crate::state`] describes *where it is*.

/// Licoman's top-level information architecture (00_START_HERE.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    Shared,
    Desktop,
    Distribution,
}

impl Scope {
    pub const ALL: [Scope; 3] = [Scope::Shared, Scope::Desktop, Scope::Distribution];

    pub fn label(self) -> &'static str {
        match self {
            Scope::Shared => "Shared",
            Scope::Desktop => "Desktop",
            Scope::Distribution => "Distribution",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Scope::Shared => "Common settings on this Linux system.",
            Scope::Desktop => "Settings provided by your desktop environment.",
            Scope::Distribution => "Distribution administration and package tooling.",
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
/// flows through commands and phases.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    Switch,
    /// Dropdown with a fixed option list.
    Choice {
        options: Vec<String>,
    },
    Slider {
        step: f64,
        unit: &'static str,
    },
    Number {
        step: f64,
        unit: &'static str,
    },
    Text {
        placeholder: &'static str,
    },
    Path,
    List,
    /// Display-only value (read-only, unavailable, or permission-held rows).
    ReadOnly,
}

/// Stable identity and metadata for one setting. `id` is permanent: search
/// results, models and providers key on it (02_FOUNDATION.md tests §2).
#[derive(Debug, Clone, PartialEq)]
pub struct SettingDescriptor {
    pub id: &'static str,
    pub scope: Scope,
    pub category: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub setting_type: crate::value::SettingType,
    pub control: Control,
    pub apply_mode: ApplyMode,
    /// Known restart requirement: applying this setting takes effect after
    /// restarting the named target. Discovered-at-write-time requirements are
    /// recorded on the model instead.
    pub restart_target: Option<&'static str>,
    /// When present, the setting is held behind an authorization boundary
    /// Licoman does not cross in this phase.
    pub permission_scope: Option<&'static str>,
    /// Provider-supplied status hint rendered as the row's status line (e.g.
    /// why a setting is read-only here). Not an error — errors live on the
    /// model.
    pub note: Option<&'static str>,
    /// Numeric bounds for Integer/Float values.
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl SettingDescriptor {
    /// Descriptor with the common metadata defaulted; mutate the result for
    /// restart targets, permission scopes, notes or numeric bounds.
    pub fn new(
        id: &'static str,
        scope: Scope,
        category: &'static str,
        label: &'static str,
        description: &'static str,
        setting_type: crate::value::SettingType,
        control: Control,
    ) -> Self {
        Self {
            id,
            scope,
            category,
            label,
            description,
            setting_type,
            control,
            apply_mode: ApplyMode::Staged,
            restart_target: None,
            permission_scope: None,
            note: None,
            min: None,
            max: None,
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
