//! TOML manifest loading and validation (04_MODULE_SCHEMA_AND_REGISTRY.md
//! §1, §4, §5).
//!
//! Two layers:
//!
//! 1. [`parse_manifest`] — TOML → [`ModuleDefinition`]. Syntax and unknown
//!    fields fail with the byte span converted to line/column.
//! 2. [`validate_definition`] — semantic validation producing **all**
//!    issues in one pass (contributor-friendliness): id formats, kind/control
//!    coherence, enum option uniqueness, range/default coherence, backend
//!    binding safety, and the data-only payload rules.
//!
//! Nothing here executes manifest content, and every string that names a
//! backend target is checked against an identifier/relative-path grammar so
//! shell metacharacters can never survive into any future consumer
//! (00_START_HERE.md non-negotiable 5).

use super::def::{
    ApplyPolicy, BackendBinding, ControlKind, FileFormat, MAX_MANIFEST_BYTES,
    MAX_SETTINGS_PER_MODULE, MODULE_SCHEMA_VERSION, ManifestValue, ModuleDefinition, ModuleScope,
    SettingDefinition, ValueKind,
};

/// Severity of one validation issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueSeverity {
    Error,
    Warning,
}

/// Where an issue sits inside a module (coarse, contributor-oriented).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueLocation {
    Module,
    Field(&'static str),
    Setting {
        id: Option<String>,
        field: &'static str,
    },
}

impl IssueLocation {
    fn describe(&self) -> String {
        match self {
            IssueLocation::Module => "module".to_string(),
            IssueLocation::Field(name) => format!("module field `{name}`"),
            IssueLocation::Setting { id, field } => match id {
                Some(id) => format!("setting `{id}` field `{field}`"),
                None => format!("setting field `{field}`"),
            },
        }
    }
}

/// One validation issue with an actionable message and, where the TOML
/// parser provided it, a 1-based source position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestIssue {
    pub severity: IssueSeverity,
    pub location: IssueLocation,
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl ManifestIssue {
    pub fn error(location: IssueLocation, message: impl Into<String>) -> Self {
        Self {
            severity: IssueSeverity::Error,
            location,
            message: message.into(),
            line: None,
            column: None,
        }
    }

    fn warning(location: IssueLocation, message: impl Into<String>) -> Self {
        Self {
            severity: IssueSeverity::Warning,
            location,
            message: message.into(),
            line: None,
            column: None,
        }
    }

    /// Human-readable one-liner for the linter and diagnostics.
    pub fn render(&self, source_name: &str) -> String {
        let position = match (self.line, self.column) {
            (Some(line), Some(col)) => format!("{source_name}:{line}:{col}: "),
            (Some(line), None) => format!("{source_name}:{line}: "),
            _ => format!("{source_name}: "),
        };
        let level = match self.severity {
            IssueSeverity::Error => "error",
            IssueSeverity::Warning => "warning",
        };
        format!(
            "{position}{level}: {} — {}",
            self.location.describe(),
            self.message
        )
    }
}

/// Parse a manifest's TOML text. Fails with located issues (parse errors,
/// unknown fields, wrong types) instead of a generic error.
pub fn parse_manifest(text: &str) -> Result<ModuleDefinition, Vec<ManifestIssue>> {
    if text.len() > MAX_MANIFEST_BYTES {
        return Err(vec![ManifestIssue::error(
            IssueLocation::Module,
            format!(
                "manifest is {} bytes; the limit is {MAX_MANIFEST_BYTES} bytes",
                text.len()
            ),
        )]);
    }
    match toml::from_str::<ModuleDefinition>(text) {
        Ok(def) => Ok(def),
        Err(error) => {
            let (line, column) = error
                .span()
                .map(|span| line_col(text, span.start))
                .unwrap_or((None, None));
            Err(vec![ManifestIssue {
                severity: IssueSeverity::Error,
                location: IssueLocation::Module,
                message: error.message().to_string(),
                line,
                column,
            }])
        }
    }
}

/// Convert a byte offset to a 1-based (line, column).
fn line_col(text: &str, offset: usize) -> (Option<usize>, Option<usize>) {
    let offset = offset.min(text.len());
    let prefix = &text[..offset];
    let line = prefix.matches('\n').count() + 1;
    let column = prefix.rsplit('\n').next().map_or(1, |rest| rest.len() + 1);
    (Some(line), Some(column))
}

// ---- identifier / path grammars -------------------------------------------

/// Lowercase namespaced identifier: `a-z0-9`, `-`, `_`, `.` separators.
/// Used for module ids, setting ids, schema names, keys, services.
fn is_identifier(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 200
        && text.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-' || c == '_'
        })
        && !text.starts_with('.')
        && !text.ends_with('.')
        && !text.contains("..")
}

/// D-Bus member (property/method) names are PascalCase by convention and
/// the D-Bus spec allows `[A-Za-z_][A-Za-z0-9_]*` — distinct from the
/// lowercase Licoman identifier grammar above.
fn is_dbus_member(text: &str) -> bool {
    !text.is_empty() && text.len() <= 200 && {
        let mut chars = text.chars();
        let first = chars.next().expect("non-empty");
        (first.is_ascii_alphabetic() || first == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
}

/// Dotted name with case-insensitive segments (D-Bus well-known names and
/// interfaces, GSettings schema ids — `org.freedesktop.portal.Desktop`).
fn is_dotted_name(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 200
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        && !text.starts_with('.')
        && !text.ends_with('.')
        && !text.contains("..")
        && text.contains('.')
}

/// Reject anything that smells like a shell target or a path escape. These
/// strings never reach a shell anywhere in Licoman — the check exists so a
/// malicious manifest fails validation instead of merely "not being
/// executed yet" (04_MODULE_SCHEMA_AND_REGISTRY.md tests §7).
fn is_shell_safe(text: &str) -> bool {
    !text.chars().any(|c| {
        matches!(
            c,
            ';' | '|' | '&' | '$' | '`' | '>' | '<' | '\n' | '\r' | '\0' | '\\' | '\'' | '"'
        )
    })
}

/// A config-root-relative file path: relative, no `..`, no absolute form,
/// no shell metacharacters, no leading/trailing slash.
fn is_safe_relative_path(text: &str) -> bool {
    if !is_shell_safe(text) || text.is_empty() || text.len() > 300 {
        return false;
    }
    if text.starts_with('/') || text.starts_with('~') || text.contains(':') {
        return false;
    }
    let mut components = 0;
    for component in text.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return false;
        }
        components += 1;
    }
    components > 0
}

// ---- validation ------------------------------------------------------------

/// Validate a parsed definition. Returns every issue found; an empty list
/// means the module is loadable. Errors reject the module; warnings do not.
pub fn validate_definition(def: &ModuleDefinition) -> Vec<ManifestIssue> {
    let mut issues = Vec::new();

    // ---- module-level ------------------------------------------------------
    if def.schema_version != MODULE_SCHEMA_VERSION {
        issues.push(ManifestIssue::error(
            IssueLocation::Field("schema_version"),
            format!(
                "unsupported module schema version {}; this build supports version {MODULE_SCHEMA_VERSION}. Update Licoman or the module — the module is rejected, never approximated.",
                def.schema_version
            ),
        ));
    }
    if !is_identifier(&def.id) || !def.id.contains('.') {
        issues.push(ManifestIssue::error(
            IssueLocation::Field("id"),
            format!(
                "module id {id:?} must be a namespaced lowercase identifier with at least two dot-separated segments (e.g. `app.alacritty`)",
                id = def.id
            ),
        ));
    } else if !def.id.starts_with(def.scope.required_prefix()) {
        issues.push(ManifestIssue::error(
            IssueLocation::Field("id"),
            format!(
                "module id {id:?} must start with {prefix:?} for its {scope:?} scope",
                id = def.id,
                prefix = def.scope.required_prefix(),
                scope = def.scope
            ),
        ));
    }
    if def.title.trim().is_empty() || def.title.len() > 200 {
        issues.push(ManifestIssue::error(
            IssueLocation::Field("title"),
            "title must be 1–200 characters".to_string(),
        ));
    }
    if def.description.len() > 1000 {
        issues.push(ManifestIssue::error(
            IssueLocation::Field("description"),
            "description must be at most 1000 characters".to_string(),
        ));
    }
    if def.category.trim().is_empty() || def.category.len() > 64 {
        issues.push(ManifestIssue::error(
            IssueLocation::Field("category"),
            "category must be 1–64 characters".to_string(),
        ));
    }
    if let Some(url) = &def.docs_url
        && (!url.starts_with("https://")
            || url.len() > 300
            || url.chars().any(|c| c.is_whitespace()))
    {
        issues.push(ManifestIssue::error(
            IssueLocation::Field("docs_url"),
            format!(
                "docs_url {url:?} must be a whitespace-free https:// URL of at most 300 characters"
            ),
        ));
    }
    if def.settings.is_empty() {
        issues.push(ManifestIssue::error(
            IssueLocation::Field("settings"),
            "module declares no settings; a module without settings has no purpose and is \
             rejected to keep contributor mistakes visible"
                .to_string(),
        ));
    }
    if def.settings.len() > MAX_SETTINGS_PER_MODULE {
        issues.push(ManifestIssue::error(
            IssueLocation::Field("settings"),
            format!(
                "module declares {} settings; the limit is {MAX_SETTINGS_PER_MODULE}",
                def.settings.len()
            ),
        ));
    }
    validate_compatibility(&def.compatibility, &mut issues);

    // ---- settings ----------------------------------------------------------
    let mut seen_ids: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for setting in &def.settings {
        issues.extend(validate_setting(def, setting));
        if !seen_ids.insert(setting.id.as_str()) {
            issues.push(ManifestIssue::error(
                IssueLocation::Setting {
                    id: Some(setting.id.clone()),
                    field: "id",
                },
                format!(
                    "duplicate setting id {id:?} within module {module:?}; ids must be unique \
                     (the module is rejected — not last-one-wins)",
                    id = setting.id,
                    module = def.id
                ),
            ));
        }
    }

    issues
}

fn validate_compatibility(compat: &super::def::Compatibility, issues: &mut Vec<ManifestIssue>) {
    for (field, list) in [
        ("desktops", &compat.desktops),
        ("distros", &compat.distros),
        ("apps", &compat.apps),
        ("required_backends", &compat.required_backends),
    ] {
        for entry in list {
            if entry.trim().is_empty() || entry.len() > 100 || !is_shell_safe(entry) {
                issues.push(ManifestIssue::error(
                    IssueLocation::Field(field),
                    format!(
                        "entry {entry:?} must be 1–100 characters and free of shell metacharacters"
                    ),
                ));
            }
        }
    }
    for backend in &compat.required_backends {
        let known = backend == "gsettings"
            || backend == "dbus"
            || backend.starts_with("config:")
            || backend.starts_with("package-manager:")
            || backend == "dbus-session"
            || backend == "gsettings-schemas";
        if !known {
            issues.push(ManifestIssue::warning(
                IssueLocation::Field("required_backends"),
                format!(
                    "backend requirement {backend:?} is not a known capability id or kind alias \
                     (gsettings, dbus, config:*, package-manager:*); it will simply not match"
                ),
            ));
        }
    }
    if let Some(range) = &compat.app_version {
        for bound in [range.min.as_deref(), range.max.as_deref()]
            .into_iter()
            .flatten()
        {
            let ok = !bound.is_empty()
                && bound.len() <= 32
                && bound
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
                && !bound.starts_with('.')
                && !bound.ends_with('.');
            if !ok {
                issues.push(ManifestIssue::error(
                    IssueLocation::Field("app_version"),
                    format!("version bound {bound:?} must look like a version number"),
                ));
            }
        }
    }
}

fn validate_setting(def: &ModuleDefinition, setting: &SettingDefinition) -> Vec<ManifestIssue> {
    let mut issues = Vec::new();
    let loc = |field: &'static str| IssueLocation::Setting {
        id: Some(setting.id.clone()),
        field,
    };
    let err = |field: &'static str, message: String| ManifestIssue::error(loc(field), message);

    // Identity and text bounds.
    if !is_identifier(&setting.id) {
        issues.push(err(
            "id",
            format!(
                "setting id {id:?} must be a lowercase identifier (a-z0-9, '-', '_', '.')",
                id = setting.id
            ),
        ));
    }
    if setting.title.trim().is_empty() || setting.title.len() > 200 {
        issues.push(err("title", "title must be 1–200 characters".to_string()));
    }
    if setting.description.len() > 1000 {
        issues.push(err(
            "description",
            "description must be at most 1000 characters".to_string(),
        ));
    }
    if setting.keywords.len() > 16 {
        issues.push(err("keywords", "at most 16 keywords".to_string()));
    }
    for keyword in &setting.keywords {
        if keyword.trim().is_empty() || keyword.len() > 64 {
            issues.push(err(
                "keywords",
                format!("keyword {keyword:?} must be 1–64 characters"),
            ));
        }
    }

    // Type is required for every control except `action`, which has no value.
    if setting.control == ControlKind::Action {
        if setting.kind.is_some() {
            issues.push(err(
                "type",
                "action settings have no value type".to_string(),
            ));
        }
        for (field, present) in [
            ("default", setting.default.is_some()),
            ("min", setting.min.is_some()),
            ("max", setting.max.is_some()),
            ("step", setting.step.is_some()),
            ("unit", setting.unit.is_some()),
            ("options", !setting.options.is_empty()),
        ] {
            if present {
                issues.push(err(
                    field,
                    format!("action settings cannot declare {field}"),
                ));
            }
        }
    } else {
        let Some(kind) = setting.kind else {
            issues.push(err(
                "type",
                format!(
                    "control `{control:?}` requires a declared value type (boolean, enum, integer, \
                     float, string or path)",
                    control = setting.control
                ),
            ));
            return issues;
        };
        issues.extend(validate_typed_setting(setting, kind));
    }

    // Policy/control coherence: read-only must be presented read-only, and a
    // read-only control must not pretend a write policy exists.
    if setting.policy == ApplyPolicy::ReadOnly && setting.control != ControlKind::ReadOnly {
        issues.push(err(
            "control",
            "policy `read_only` requires control `read_only` so the UI never renders a \
             control that cannot write"
                .to_string(),
        ));
    }
    if setting.control == ControlKind::ReadOnly && setting.policy != ApplyPolicy::ReadOnly {
        issues.push(err(
            "policy",
            "control `read_only` requires policy `read_only`".to_string(),
        ));
    }
    // Editable controls are staged/immediate only; the store enforces the rest.
    if setting.control.is_editable() && setting.policy == ApplyPolicy::ReadOnly {
        issues.push(err(
            "policy",
            "editable controls cannot be read_only".to_string(),
        ));
    }

    // Auth, restart, risk fields are free text with bounds.
    if let Some(auth) = &setting.required_auth
        && (auth.trim().is_empty() || auth.len() > 100 || !is_shell_safe(auth))
    {
        issues.push(err(
            "required_auth",
            format!("required_auth {auth:?} must be 1–100 characters without shell metacharacters"),
        ));
    }
    if let Some(restart) = &setting.restart
        && (restart.trim().is_empty() || restart.len() > 100)
    {
        issues.push(err(
            "restart",
            "restart target must be 1–100 characters".to_string(),
        ));
    }
    if let Some(unit) = &setting.unit
        && unit.len() > 16
    {
        issues.push(err(
            "unit",
            "unit must be at most 16 characters".to_string(),
        ));
    }
    if let Some(group) = &setting.group
        && (group.trim().is_empty() || group.len() > 64)
    {
        issues.push(err("group", "group must be 1–64 characters".to_string()));
    }

    // Availability predicate and backend binding.
    if let Some(predicate) = &setting.availability {
        for (field, value) in [
            ("desktop", predicate.desktop.as_deref()),
            ("distro", predicate.distro.as_deref()),
            ("app", predicate.app.as_deref()),
            ("backend", predicate.backend.as_deref()),
        ] {
            if let Some(value) = value
                && (value.trim().is_empty() || value.len() > 100 || !is_shell_safe(value))
            {
                issues.push(err(
                    field,
                    format!("availability.{field} {value:?} must be 1–100 characters"),
                ));
            }
        }
    }
    issues.extend(validate_backend(def, setting));

    issues
}

/// Kind-specific coherence: options, ranges, defaults, nullability.
fn validate_typed_setting(setting: &SettingDefinition, kind: ValueKind) -> Vec<ManifestIssue> {
    use ControlKind as CK;
    let mut issues = Vec::new();
    let loc = |field: &'static str| IssueLocation::Setting {
        id: Some(setting.id.clone()),
        field,
    };
    let err = |field: &'static str, message: String| ManifestIssue::error(loc(field), message);

    // Control ↔ kind coherence. `read_only` is a display control valid for
    // any typed value (informational rows).
    let control_ok = match kind {
        ValueKind::Boolean => matches!(setting.control, CK::Switch | CK::Checkbox),
        ValueKind::Enum => setting.control == CK::Dropdown,
        ValueKind::Integer | ValueKind::Float => {
            matches!(setting.control, CK::Number | CK::Slider)
        }
        ValueKind::String => setting.control == CK::Text,
        ValueKind::Path => setting.control == CK::PathInput,
        // Lists render through the read-only path (service-provided
        // collections; no editing in this phase).
        ValueKind::List => matches!(setting.control, CK::ReadOnly),
    } || setting.control == CK::ReadOnly;
    if !control_ok {
        issues.push(err(
            "control",
            format!(
                "control `{control:?}` cannot hold values of type {kind:?}",
                control = setting.control
            ),
        ));
    }

    // Enum options.
    if kind == ValueKind::Enum {
        if setting.options.is_empty() {
            issues.push(err(
                "options",
                "enum settings must declare options".to_string(),
            ));
        } else if setting.options.len() > 64 {
            issues.push(err("options", "at most 64 enum options".to_string()));
        }
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for option in &setting.options {
            if option.value.trim().is_empty() || option.value.len() > 100 {
                issues.push(err(
                    "options",
                    format!(
                        "option value {value:?} must be 1–100 characters",
                        value = option.value
                    ),
                ));
            }
            if !seen.insert(option.value.as_str()) {
                issues.push(err(
                    "options",
                    format!("duplicate option value {value:?}", value = option.value),
                ));
            }
            if let Some(label) = &option.label
                && (label.trim().is_empty() || label.len() > 100)
            {
                issues.push(err(
                    "options",
                    format!("option label {label:?} must be 1–100 characters"),
                ));
            }
        }
    }

    // Numeric bounds and step.
    if matches!(kind, ValueKind::Integer | ValueKind::Float) {
        if let (Some(min), Some(max)) = (setting.min, setting.max)
            && (!min.is_finite() || !max.is_finite() || min > max)
        {
            issues.push(err(
                "min",
                format!("invalid range: min {min:?} must be ≤ max {max:?} and both finite"),
            ));
        }
        if let Some(step) = setting.step
            && (!step.is_finite() || step <= 0.)
        {
            issues.push(err(
                "step",
                format!("step {step:?} must be a positive number"),
            ));
        }
        if setting.nullable {
            issues.push(err(
                "nullable",
                "only string/path settings may be nullable".to_string(),
            ));
        }
    } else {
        for (field, present) in [
            ("min", setting.min.is_some()),
            ("max", setting.max.is_some()),
            ("step", setting.step.is_some()),
        ] {
            if present {
                issues.push(err(
                    field,
                    format!("{field} only applies to number/slider settings"),
                ));
            }
        }
    }

    // String/path length bounds and nullability.
    if matches!(kind, ValueKind::String | ValueKind::Path) {
        if let Some(max_length) = setting.max_length
            && (max_length == 0 || max_length > 4096)
        {
            issues.push(err(
                "max_length",
                format!("max_length {max_length} must be 1–4096"),
            ));
        }
    } else if setting.max_length.is_some() {
        issues.push(err(
            "max_length",
            "max_length only applies to string/path settings".to_string(),
        ));
    }
    if setting.nullable && !matches!(kind, ValueKind::String | ValueKind::Path) {
        issues.push(err(
            "nullable",
            "only string/path settings may be nullable".to_string(),
        ));
    }

    // Default value coherence.
    if let Some(default) = &setting.default {
        issues.extend(validate_default(setting, kind, default));
    } else if matches!(
        kind,
        ValueKind::Boolean | ValueKind::Enum | ValueKind::Integer | ValueKind::Float
    ) && setting.backend.is_mock()
    {
        issues.push(err(
            "default",
            "mock-backed settings of this type must declare a default; the mock value source has \
             nothing else to serve and would have to invent one"
                .to_string(),
        ));
    }
    issues
}

fn validate_default(
    setting: &SettingDefinition,
    kind: ValueKind,
    default: &ManifestValue,
) -> Vec<ManifestIssue> {
    let mut issues = Vec::new();
    let loc = |field: &'static str| IssueLocation::Setting {
        id: Some(setting.id.clone()),
        field,
    };
    let err = |field: &'static str, message: String| ManifestIssue::error(loc(field), message);

    let type_ok = matches!(
        (kind, default),
        (ValueKind::Boolean, ManifestValue::Bool(_))
            | (ValueKind::Enum, ManifestValue::Str(_))
            | (ValueKind::Integer, ManifestValue::Int(_))
            | (
                ValueKind::Float,
                ManifestValue::Int(_) | ManifestValue::Float(_)
            )
            | (ValueKind::String, ManifestValue::Str(_))
            | (ValueKind::Path, ManifestValue::Str(_))
    );
    if !type_ok {
        issues.push(err(
            "default",
            format!(
                "default {default:?} does not match the declared type {kind:?}",
                kind = kind
            ),
        ));
        return issues;
    }

    // Range check for numeric defaults.
    let numeric = match default {
        ManifestValue::Int(n) => Some(*n as f64),
        ManifestValue::Float(n) => Some(*n),
        _ => None,
    };
    if let Some(value) = numeric {
        if let Some(min) = setting.min
            && value < min
        {
            issues.push(err(
                "default",
                format!("default {value} is below the declared minimum {min}"),
            ));
        }
        if let Some(max) = setting.max
            && value > max
        {
            issues.push(err(
                "default",
                format!("default {value} is above the declared maximum {max}"),
            ));
        }
    }

    // Enum membership.
    if kind == ValueKind::Enum
        && let ManifestValue::Str(choice) = default
        && !setting.options.iter().any(|o| o.value == *choice)
    {
        issues.push(err(
            "default",
            format!("default {choice:?} is not one of the declared enum options"),
        ));
    }

    // Path defaults must be safe relative paths (config-root relative).
    // An empty path default on a nullable setting means "not set".
    if kind == ValueKind::Path
        && let ManifestValue::Str(path) = default
        && !path.is_empty()
        && !is_safe_relative_path(path)
    {
        issues.push(err(
            "default",
            format!(
                "path default {path:?} must be a relative path under the configuration \
                         root (no absolute paths, no `..`, no shell metacharacters); an empty \
                         value is only allowed for nullable settings and means \"not set\""
            ),
        ));
    }
    issues
}

fn validate_backend(def: &ModuleDefinition, setting: &SettingDefinition) -> Vec<ManifestIssue> {
    let mut issues = Vec::new();
    let loc = |field: &'static str| IssueLocation::Setting {
        id: Some(setting.id.clone()),
        field,
    };
    let err = |field: &'static str, message: String| ManifestIssue::error(loc(field), message);

    match &setting.backend {
        BackendBinding::Mock => {
            if def.scope != ModuleScope::Shared
                && def.scope != ModuleScope::Desktop
                && def.scope != ModuleScope::Distribution
                && def.scope != ModuleScope::Application
            {
                // Unreachable (all scopes listed); kept for exhaustiveness.
            }
            // Mock bindings serve declared defaults; nothing to verify here.
        }
        BackendBinding::GSettings { schema, key } => {
            if !is_dotted_name(schema) {
                issues.push(err(
                    "backend",
                    format!("GSettings schema {schema:?} must be a dotted lowercase schema id"),
                ));
            }
            if !is_identifier(key) || key.contains('.') {
                issues.push(err(
                    "backend",
                    format!("GSettings key {key:?} must be a lowercase key name without dots"),
                ));
            }
        }
        BackendBinding::DBus {
            service,
            path,
            interface,
            property,
        } => {
            if !is_dotted_name(service) {
                issues.push(err(
                    "backend",
                    format!("D-Bus service {service:?} must be a dotted well-known name"),
                ));
            }
            if !path.starts_with('/')
                || !is_shell_safe(path)
                || path.contains("..")
                || path.len() > 300
                || path.contains("//")
            {
                issues.push(err(
                    "backend",
                    format!("D-Bus object path {path:?} must be a clean absolute object path"),
                ));
            }
            if !is_dotted_name(interface) {
                issues.push(err(
                    "backend",
                    format!("D-Bus interface {interface:?} must be a dotted interface name"),
                ));
            }
            if let Some(property) = property
                && !(is_dbus_member(property) || is_identifier(property))
            {
                issues.push(err(
                    "backend",
                    format!("D-Bus property {property:?} must be an identifier"),
                ));
            }
        }
        BackendBinding::File {
            path, format, key, ..
        } => {
            if !is_safe_relative_path(path) {
                issues.push(err(
                    "backend",
                    format!(
                        "file path {path:?} must be relative to the configuration root (no \
                         absolute paths, no `..`, no shell metacharacters, no drive/URI prefixes)"
                    ),
                ));
            }
            if !matches!(
                format,
                FileFormat::Toml | FileFormat::Json | FileFormat::Keyfile
            ) {
                issues.push(err("backend", "unsupported file format".to_string()));
            }
            if let Some(key) = key
                && (key.trim().is_empty() || key.len() > 200 || !is_shell_safe(key))
            {
                issues.push(err(
                    "backend",
                    format!(
                        "file key {key:?} must be 1–200 characters without shell metacharacters"
                    ),
                ));
            }
        }
    }
    issues
}

// ---- tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
schema_version = 1
id = "desktop.gnome.interface"
scope = "desktop"
title = "GNOME Interface"
description = "Appearance and behavior settings provided by GNOME."
category = "Appearance"
docs_url = "https://docs.gtk.org/gio/class.Settings.html"

[compatibility]
desktops = ["GNOME"]
required_backends = ["gsettings"]

[support]
maintainer = "Licoman maintainers"
level = "supported"

[[settings]]
id = "clock.show_weekday"
title = "Show weekday in clock"
description = "Display the weekday in the top bar clock."
keywords = ["clock", "date", "weekday", "top bar"]
type = "boolean"
control = "switch"
policy = "immediate"

[settings.backend]
kind = "gsettings"
schema = "org.gnome.desktop.interface"
key = "clock-show-weekday"
"#;

    #[test]
    fn valid_plan_example_parses_and_validates() {
        let def = parse_manifest(VALID).expect("parses");
        assert_eq!(def.id, "desktop.gnome.interface");
        assert_eq!(def.settings.len(), 1);
        let issues = validate_definition(&def);
        assert_eq!(issues, Vec::new(), "unexpected issues: {issues:?}");
    }

    #[test]
    fn unknown_field_is_rejected_with_location() {
        let text = VALID.replace(
            "category = \"Appearance\"",
            "category = \"Appearance\"\ncolour = \"red\"",
        );
        let issues = parse_manifest(&text).unwrap_err();
        assert!(issues[0].message.contains("unknown field"), "{issues:?}");
        assert!(issues[0].line.is_some(), "parse issues carry a line");
    }

    #[test]
    fn unsupported_schema_version_is_an_explicit_rejection() {
        let text = VALID.replace("schema_version = 1", "schema_version = 2");
        let def = parse_manifest(&text).expect("parses");
        let issues = validate_definition(&def);
        assert!(
            issues
                .iter()
                .any(|i| i.severity == IssueSeverity::Error
                    && i.message.contains("schema version 2")),
            "{issues:?}"
        );
    }

    #[test]
    fn unknown_control_and_backend_kinds_are_rejected() {
        let text = VALID.replace("control = \"switch\"", "control = \"turbo_encabulator\"");
        let issues = parse_manifest(&text).unwrap_err();
        assert!(issues[0].message.contains("unknown variant"), "{issues:?}");

        let text = VALID.replace("kind = \"gsettings\"", "kind = \"exec\"");
        let issues = parse_manifest(&text).unwrap_err();
        assert!(issues[0].message.contains("unknown variant"), "{issues:?}");
    }

    #[test]
    fn duplicate_setting_ids_are_rejected_not_last_one_wins() {
        let extra = VALID.replace(
            "[[settings]]",
            "[[settings]]\nid = \"clock.show_weekday\"\ntitle = \"Dup\"\ntype = \"boolean\"\ncontrol = \"switch\"\n\n[settings.backend]\nkind = \"gsettings\"\nschema = \"org.gnome.desktop.interface\"\nkey = \"clock-show-seconds\"\n\n[[settings]]",
        );
        let def = parse_manifest(&extra).expect("parses");
        let issues = validate_definition(&def);
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("duplicate setting id")),
            "{issues:?}"
        );
    }

    #[test]
    fn invalid_range_and_default_types_are_field_level_errors() {
        let text = VALID.replace(
            "type = \"boolean\"\ncontrol = \"switch\"",
            "type = \"integer\"\ncontrol = \"slider\"\nmin = 100.0\nmax = 0.0\ndefault = 200",
        );
        let def = parse_manifest(&text).expect("parses");
        let issues = validate_definition(&def);
        assert!(
            issues.iter().any(|i| i.message.contains("invalid range")),
            "{issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("above the declared maximum")),
            "{issues:?}"
        );
    }

    #[test]
    fn malicious_path_and_shell_payloads_are_rejected() {
        for payload in [
            "../../etc/passwd",
            "/home/user/.config/app.conf",
            "config; rm -rf ~",
            "a`id`b",
            "x$(id)y",
            "~/.ssh/config",
        ] {
            let text = VALID.replace(
                "kind = \"gsettings\"\nschema = \"org.gnome.desktop.interface\"\nkey = \"clock-show-weekday\"",
                &format!("kind = \"file\"\npath = \"{payload}\"\nformat = \"toml\""),
            );
            let def = parse_manifest(&text).expect("parses");
            let issues = validate_definition(&def);
            assert!(
                issues.iter().any(|i| i.severity == IssueSeverity::Error),
                "payload {payload:?} must be rejected: {issues:?}"
            );
        }
    }

    #[test]
    fn mock_backed_boolean_requires_a_default() {
        let text = VALID
            .replace("desktops = [\"GNOME\"]\nrequired_backends = [\"gsettings\"]", "")
            .replace(
                "kind = \"gsettings\"\nschema = \"org.gnome.desktop.interface\"\nkey = \"clock-show-weekday\"",
                "kind = \"mock\"",
            )
            .replace("id = \"desktop.gnome.interface\"", "id = \"desktop.mock.things\"");
        let def = parse_manifest(&text).expect("parses");
        let issues = validate_definition(&def);
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("must declare a default")),
            "{issues:?}"
        );
    }

    #[test]
    fn oversized_manifest_is_rejected_upfront() {
        let filler = "x = 1\n".repeat(MAX_MANIFEST_BYTES / 6 + 10);
        let issues = parse_manifest(&filler).unwrap_err();
        assert!(issues[0].message.contains("limit"), "{issues:?}");
    }
}
