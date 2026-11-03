//! Applicability and compatibility evaluation (04_MODULE_SCHEMA_AND_REGISTRY.md
//! §4): "Query EnvironmentSnapshot and provider capabilities → Compute
//! applicability and compatibility reasons."
//!
//! Evaluation is a **pure function over [`EnvironmentFacts`]** — the grounded
//! facts extracted from a discovery snapshot (desktop identity, distro ids,
//! installed app ids, backend capability ids, GSettings schema/key sets,
//! session-bus names). No I/O, no probing, deterministic for given facts, so
//! tests can state exact reasons.
//!
//! Every incompatibility dimension produces a **distinct, preserved reason**
//! (schema version, desktop, distro, application, backend, predicate) —
//! diagnostics are data, not a single opaque "incompatible".

use std::collections::{BTreeMap, BTreeSet};

use super::def::{AvailabilityPredicate, BackendBinding, ModuleDefinition};
use crate::descriptor::Control;
use crate::discovery::{BackendStatus, DesktopIdentity, EnvironmentSnapshot};

/// Whether a module's applicability has been (or can be) evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplicabilityState {
    /// No environment facts yet (discovery pending). Not applicable *and not
    /// claimed incompatible* — the UI shows a pending state, never a guess.
    Pending,
    Applicable,
    Inapplicable,
}

/// The dimension an incompatibility reason belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncompatibilityDimension {
    Desktop,
    Distro,
    Application,
    Backend,
    Predicate,
}

impl IncompatibilityDimension {
    pub fn as_str(self) -> &'static str {
        match self {
            IncompatibilityDimension::Desktop => "desktop",
            IncompatibilityDimension::Distro => "distro",
            IncompatibilityDimension::Application => "application",
            IncompatibilityDimension::Backend => "backend",
            IncompatibilityDimension::Predicate => "predicate",
        }
    }
}

/// One preserved incompatibility reason (user-safe message).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncompatibilityReason {
    pub dimension: IncompatibilityDimension,
    pub message: String,
}

/// Result of evaluating a module against environment facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleApplicability {
    pub state: ApplicabilityState,
    pub reasons: Vec<IncompatibilityReason>,
}

impl ModuleApplicability {
    pub fn pending() -> Self {
        Self {
            state: ApplicabilityState::Pending,
            reasons: Vec::new(),
        }
    }

    pub fn is_available(&self) -> bool {
        self.state == ApplicabilityState::Applicable
    }

    /// Joined user-safe reason text (used for unavailable rows).
    pub fn reason_text(&self) -> Option<String> {
        if self.reasons.is_empty() {
            return None;
        }
        Some(
            self.reasons
                .iter()
                .map(|r| r.message.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        )
    }
}

/// The environment facts applicability evaluation needs. Extracted once from
/// a snapshot (cheap to clone) so evaluation stays pure.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EnvironmentFacts {
    pub desktop: DesktopIdentity,
    pub distro_id: Option<String>,
    pub distro_family: Vec<String>,
    /// Lowercased desktop-file ids without the `.desktop` suffix.
    pub installed_app_ids: BTreeSet<String>,
    /// Available backend capability ids (e.g. `gsettings-schemas`,
    /// `dbus-session`, `config:alacritty`).
    pub backend_ids: BTreeSet<String>,
    /// GSettings schemas with their declared key names.
    pub gsettings_schemas: BTreeMap<String, BTreeSet<String>>,
    /// Well-known names visible on the session bus.
    pub bus_names: BTreeSet<String>,
    /// Well-known names visible on the system bus (Phase 05).
    pub bus_names_system: BTreeSet<String>,
}

impl EnvironmentFacts {
    pub fn from_snapshot(snapshot: &EnvironmentSnapshot) -> Self {
        Self {
            desktop: snapshot.session.desktop,
            distro_id: snapshot.distro.id.clone(),
            distro_family: snapshot.distro.id_like.clone(),
            installed_app_ids: snapshot
                .installed_apps
                .iter()
                .map(|app| normalize_app_id(&app.id))
                .collect(),
            backend_ids: snapshot
                .available_backends
                .iter()
                .filter(|b| b.status == BackendStatus::Available)
                .map(|b| b.id.clone())
                .collect(),
            gsettings_schemas: snapshot.gsettings_schemas.clone(),
            bus_names: snapshot.bus_names.clone(),
            bus_names_system: snapshot.bus_names_system.clone(),
        }
    }
}

/// Desktop-file id normalization: lowercase, `.desktop` suffix stripped.
pub fn normalize_app_id(id: &str) -> String {
    let lower = id.to_ascii_lowercase();
    lower.strip_suffix(".desktop").unwrap_or(&lower).to_string()
}

/// Case-insensitive desktop-identity match with the aliases users and
/// modules actually write. `Unknown` matches nothing — a module requiring a
/// desktop on an unidentified session is conservatively inapplicable, with
/// the uncertainty stated.
pub fn desktop_matches(identity: DesktopIdentity, required: &str) -> bool {
    if identity == DesktopIdentity::Unknown {
        return false;
    }
    let aliases: &[&str] = match identity {
        DesktopIdentity::Unknown => &[],
        DesktopIdentity::Gnome => &["gnome"],
        DesktopIdentity::Kde => &["kde", "kde plasma", "plasma"],
        DesktopIdentity::Xfce => &["xfce"],
        DesktopIdentity::Cosmic => &["cosmic"],
        DesktopIdentity::Hyprland => &["hyprland"],
        DesktopIdentity::Sway => &["sway"],
    };
    let needle = required.trim().to_ascii_lowercase();
    aliases.contains(&needle.as_str())
}

/// Distribution family match over os-release `ID` and `ID_LIKE`
/// (case-insensitive; delegates to the Phase 03 evidence type when a
/// snapshot distribution is available).
pub fn distro_matches(facts: &EnvironmentFacts, required: &str) -> bool {
    let needle = required.trim().to_ascii_lowercase();
    let mut ids: Vec<&str> = facts.distro_family.iter().map(String::as_str).collect();
    if let Some(id) = &facts.distro_id {
        ids.push(id);
    }
    ids.iter().any(|id| id.eq_ignore_ascii_case(&needle))
}

/// Backend capability check with kind aliases: `gsettings` →
/// `gsettings-schemas`, `dbus` → `dbus-session`; everything else matches a
/// capability id exactly.
pub fn capability_available(required: &str, facts: &EnvironmentFacts) -> bool {
    let resolved = match required {
        "gsettings" => "gsettings-schemas",
        "dbus" => "dbus-session",
        other => other,
    };
    facts.backend_ids.contains(resolved)
}

/// Evaluate a module's compatibility. `None` facts → [`ApplicabilityState::Pending`].
pub fn evaluate(def: &ModuleDefinition, facts: Option<&EnvironmentFacts>) -> ModuleApplicability {
    let Some(facts) = facts else {
        return ModuleApplicability::pending();
    };
    let compat = &def.compatibility;
    let mut reasons = Vec::new();

    if !compat.desktops.is_empty() {
        match facts.desktop {
            DesktopIdentity::Unknown => reasons.push(IncompatibilityReason {
                dimension: IncompatibilityDimension::Desktop,
                message: format!(
                    "Requires {} — but the desktop environment could not be identified on this \
                     system, so the module is conservatively not enabled.",
                    join_or(&compat.desktops)
                ),
            }),
            detected => {
                if !compat
                    .desktops
                    .iter()
                    .any(|required| desktop_matches(detected, required))
                {
                    reasons.push(IncompatibilityReason {
                        dimension: IncompatibilityDimension::Desktop,
                        message: format!(
                            "Requires {} — this system runs {}.",
                            join_or(&compat.desktops),
                            desktop_name(detected)
                        ),
                    });
                }
            }
        }
    }

    if !compat.distros.is_empty() && !compat.distros.iter().any(|d| distro_matches(facts, d)) {
        reasons.push(IncompatibilityReason {
            dimension: IncompatibilityDimension::Distro,
            message: format!(
                "Requires distribution {} — {}.",
                join_or(&compat.distros),
                facts
                    .distro_id
                    .as_deref()
                    .map(|id| format!("this system identifies as {id:?}"))
                    .unwrap_or_else(|| "the distribution could not be identified".to_string())
            ),
        });
    }

    if !compat.apps.is_empty() {
        let any_installed = compat
            .apps
            .iter()
            .any(|app| facts.installed_app_ids.contains(&normalize_app_id(app)));
        if !any_installed {
            reasons.push(IncompatibilityReason {
                dimension: IncompatibilityDimension::Application,
                message: format!(
                    "Requires {} — none of them is installed on this system.",
                    join_or(&compat.apps)
                ),
            });
        }
    }

    if compat.app_version.is_some() {
        reasons.push(IncompatibilityReason {
            dimension: IncompatibilityDimension::Application,
            message: "Declares an application version requirement — Licoman cannot detect \
                      application versions yet, so the module is conservatively treated as \
                      incompatible instead of guessing."
                .to_string(),
        });
    }

    for required in &compat.required_backends {
        if !capability_available(required, facts) {
            reasons.push(IncompatibilityReason {
                dimension: IncompatibilityDimension::Backend,
                message: format!(
                    "Requires backend capability {required:?}, which is not available on this \
                     system."
                ),
            });
        }
    }

    if reasons.is_empty() {
        ModuleApplicability {
            state: ApplicabilityState::Applicable,
            reasons,
        }
    } else {
        ModuleApplicability {
            state: ApplicabilityState::Inapplicable,
            reasons,
        }
    }
}

/// Per-setting availability: the binding target must actually exist in the
/// environment (a missing GSettings schema or a stopped service makes the
/// setting unavailable — never a fake value). The module must already be
/// applicable; predicate and binding checks are per-setting.
pub fn setting_status(
    module: &ModuleDefinition,
    setting: &super::def::SettingDefinition,
    facts: Option<&EnvironmentFacts>,
) -> Result<(), String> {
    let Some(facts) = facts else {
        return Err(
            "Environment discovery has not finished yet; availability is being determined."
                .to_string(),
        );
    };
    if let Some(predicate) = &setting.availability {
        evaluate_predicate(predicate, facts)
            .map_err(|message| format!("Setting {setting}: {message}", setting = setting.title))?;
    }
    match &setting.backend {
        BackendBinding::Mock => Ok(()),
        BackendBinding::GSettings { schema, key } => match facts.gsettings_schemas.get(schema) {
            None => Err(format!(
                "The GSettings schema {schema:?} is not present on this system, so this \
                     setting is unavailable (no value is faked)."
            )),
            Some(keys) if !keys.contains(key) => Err(format!(
                "The GSettings schema {schema:?} exists but declares no key {key:?}."
            )),
            Some(_) => Ok(()),
        },
        BackendBinding::DBus { service, .. } => {
            // Services are probed passively on both buses; a service is
            // available when its name is visible on the bus it runs on.
            if facts.bus_names.contains(service) || facts.bus_names_system.contains(service) {
                Ok(())
            } else {
                Err(format!(
                    "The D-Bus service {service:?} was not found on the session or system \
                     bus. Services are probed passively; Licoman never starts one to make \
                     a setting look available."
                ))
            }
        }
        BackendBinding::Xfconf { .. } => {
            // The xfconfd service name must be visible on the session bus
            // (the same passive probe every service check uses). Channel and
            // property presence are validated per row by the backend's own
            // probe — the facts here carry names only.
            if facts
                .bus_names
                .contains(crate::read::xfconf::XFCONF_SERVICE)
            {
                Ok(())
            } else {
                Err(format!(
                    "The Xfconf configuration service ({}) was not found on the session \
                     bus. It ships with Xfce; Licoman never starts one to make a setting \
                     look available.",
                    crate::read::xfconf::XFCONF_SERVICE
                ))
            }
        }
        BackendBinding::File { .. } => {
            // Presence of the file itself is a Phase 05 read concern; module
            // compatibility carries the required capability (e.g.
            // `config:<app>`). Nothing further to verify here.
            let _ = module;
            Ok(())
        }
    }
}

fn evaluate_predicate(
    predicate: &AvailabilityPredicate,
    facts: &EnvironmentFacts,
) -> Result<(), String> {
    if let Some(desktop) = &predicate.desktop
        && !desktop_matches(facts.desktop, desktop)
    {
        return Err(format!(
            "requires desktop environment {desktop:?} (detected: {})",
            desktop_name(facts.desktop)
        ));
    }
    if let Some(distro) = &predicate.distro
        && !distro_matches(facts, distro)
    {
        return Err(format!("requires distribution {distro:?}"));
    }
    if let Some(app) = &predicate.app
        && !facts.installed_app_ids.contains(&normalize_app_id(app))
    {
        return Err(format!("requires installed application {app:?}"));
    }
    if let Some(backend) = &predicate.backend
        && !capability_available(backend, facts)
    {
        return Err(format!("requires backend capability {backend:?}"));
    }
    Ok(())
}

fn join_or(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 => format!("{:?}", items[0]),
        _ => {
            let listed: Vec<String> = items.iter().map(|i| format!("{i:?}")).collect();
            listed.join(" or ")
        }
    }
}

fn desktop_name(identity: DesktopIdentity) -> &'static str {
    match identity {
        DesktopIdentity::Unknown => "an unidentified desktop",
        DesktopIdentity::Gnome => "GNOME",
        DesktopIdentity::Kde => "KDE Plasma",
        DesktopIdentity::Xfce => "Xfce",
        DesktopIdentity::Cosmic => "COSMIC",
        DesktopIdentity::Hyprland => "Hyprland",
        DesktopIdentity::Sway => "Sway",
    }
}

/// Descriptor conversion helper: the truthful note for settings whose
/// backend is not mock-backed (they render read-only in this phase).
pub fn backend_note(binding: &BackendBinding) -> Option<String> {
    if binding.is_mock() {
        return None;
    }
    Some(format!(
        "Bound to a {} backend — read support follows the backend's documented limits; \
         this build never writes it without the safe write engine.",
        binding.kind_name()
    ))
}

/// Descriptor conversion helper: read-only rendering for non-mock backends.
pub fn descriptor_control(control: super::def::ControlKind, binding: &BackendBinding) -> Control {
    use super::def::ControlKind as CK;
    if !binding.is_mock() {
        return Control::ReadOnly;
    }
    match control {
        CK::Switch => Control::Switch,
        CK::Checkbox => Control::Switch,
        CK::Dropdown => Control::Choice {
            options: Vec::new(),
        }, // options filled by caller
        CK::Number => Control::Number {
            step: 1.,
            unit: String::new(),
        },
        CK::Slider => Control::Slider {
            step: 1.,
            unit: String::new(),
        },
        CK::Text => Control::Text {
            placeholder: String::new(),
        },
        CK::PathInput => Control::Path,
        CK::Action => Control::Action,
        CK::ReadOnly => Control::ReadOnly,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{BackendCapability, BackendKind, InstallOrigin, InstalledApp};
    use crate::modules::Compatibility;
    use std::path::PathBuf;

    fn app(id: &str) -> InstalledApp {
        InstalledApp {
            id: id.to_string(),
            name: id.to_string(),
            generic_name: None,
            comment: None,
            icon: None,
            origin: InstallOrigin::System,
            source_path: PathBuf::from(format!("/usr/share/applications/{id}")),
            no_display: false,
            try_exec_found: true,
            restricted_from_current: false,
            wm_class: None,
            flatpak_id: None,
            evidence: Vec::new(),
        }
    }

    fn facts() -> EnvironmentFacts {
        EnvironmentFacts {
            desktop: DesktopIdentity::Hyprland,
            distro_id: Some("arch".to_string()),
            distro_family: vec!["arch".to_string()],
            installed_app_ids: ["alacritty", "kitty"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            backend_ids: [
                "gsettings-schemas",
                "dbus-session",
                "dbus-system",
                "config:alacritty",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            bus_names_system: Default::default(),
            gsettings_schemas: [(
                "org.gnome.desktop.interface".to_string(),
                ["clock-show-weekday".to_string()].into_iter().collect(),
            )]
            .into_iter()
            .collect(),
            bus_names: ["org.freedesktop.portal.Desktop".to_string()]
                .into_iter()
                .collect(),
        }
    }

    fn module(compat: Compatibility) -> ModuleDefinition {
        let mut def = crate::modules::manifest::parse_manifest(
            r#"
schema_version = 1
id = "desktop.test.module"
scope = "desktop"
title = "Test"
category = "Test"

[[settings]]
id = "one"
title = "One"
type = "boolean"
control = "switch"
default = true

[settings.backend]
kind = "mock"
"#,
        )
        .unwrap();
        def.compatibility = compat;
        def
    }

    fn snapshot(f: &EnvironmentFacts) -> EnvironmentSnapshot {
        EnvironmentSnapshot {
            distro: Default::default(),
            kernel: None,
            execution_context: Default::default(),
            session: Default::default(),
            installed_apps: vec![],
            available_backends: vec![],
            problems: vec![],
            gsettings_schemas: f.gsettings_schemas.clone(),
            bus_names: f.bus_names.clone(),
            bus_names_system: f.bus_names_system.clone(),
        }
    }

    #[test]
    fn pending_without_facts_and_applicable_with_matching_facts() {
        let def = module(Compatibility {
            desktops: vec!["Hyprland".into()],
            distros: vec!["arch".into()],
            required_backends: vec!["gsettings".into()],
            ..Compatibility::default()
        });
        assert_eq!(evaluate(&def, None).state, ApplicabilityState::Pending);
        let result = evaluate(&def, Some(&facts()));
        assert_eq!(result.state, ApplicabilityState::Applicable, "{result:?}");
    }

    #[test]
    fn desktop_mismatch_is_a_distinct_preserved_reason() {
        let def = module(Compatibility {
            desktops: vec!["GNOME".into()],
            ..Compatibility::default()
        });
        let result = evaluate(&def, Some(&facts()));
        assert_eq!(result.state, ApplicabilityState::Inapplicable);
        assert_eq!(result.reasons.len(), 1);
        assert_eq!(
            result.reasons[0].dimension,
            IncompatibilityDimension::Desktop
        );
        assert!(result.reasons[0].message.contains("Hyprland"));
    }

    #[test]
    fn unknown_desktop_is_conservative_with_uncertainty_explained() {
        let mut f = facts();
        f.desktop = DesktopIdentity::Unknown;
        let def = module(Compatibility {
            desktops: vec!["GNOME".into()],
            ..Compatibility::default()
        });
        let result = evaluate(&def, Some(&f));
        assert!(
            result.reasons[0]
                .message
                .contains("could not be identified")
        );
    }

    #[test]
    fn distro_family_match_uses_id_and_id_like() {
        let def = module(Compatibility {
            distros: vec!["arch".into()],
            ..Compatibility::default()
        });
        assert_eq!(
            evaluate(&def, Some(&facts())).state,
            ApplicabilityState::Applicable
        );
        let def = module(Compatibility {
            distros: vec!["debian".into()],
            ..Compatibility::default()
        });
        let result = evaluate(&def, Some(&facts()));
        assert_eq!(
            result.reasons[0].dimension,
            IncompatibilityDimension::Distro
        );
    }

    #[test]
    fn app_not_installed_and_version_unknown_are_distinct_reasons() {
        let def = module(Compatibility {
            apps: vec!["mpv.desktop".into()],
            app_version: Some(super::super::def::VersionRange {
                min: Some("1.0".into()),
                max: None,
            }),
            ..Compatibility::default()
        });
        let result = evaluate(&def, Some(&facts()));
        assert_eq!(result.reasons.len(), 2);
        assert!(
            result
                .reasons
                .iter()
                .any(|r| r.message.contains("none of them is installed"))
        );
        assert!(
            result
                .reasons
                .iter()
                .any(|r| r.message.contains("cannot detect application versions"))
        );

        // Installed app (suffix/case-insensitive) clears the app reason.
        let def = module(Compatibility {
            apps: vec!["Alacritty.desktop".into()],
            ..Compatibility::default()
        });
        let result = evaluate(&def, Some(&facts()));
        assert_eq!(result.state, ApplicabilityState::Applicable);
    }

    #[test]
    fn missing_backend_capability_is_reported_with_its_reason() {
        let mut f = facts();
        f.backend_ids.remove("gsettings-schemas");
        let def = module(Compatibility {
            required_backends: vec!["gsettings".into()],
            ..Compatibility::default()
        });
        let result = evaluate(&def, Some(&f));
        assert!(
            result
                .reasons
                .iter()
                .any(|r| r.dimension == IncompatibilityDimension::Backend),
            "{result:?}"
        );
    }

    #[test]
    fn missing_gsettings_schema_or_key_makes_the_setting_unavailable() {
        let f = facts();
        let binding = BackendBinding::GSettings {
            schema: "org.gnome.desktop.interface".into(),
            key: "clock-show-weekday".into(),
        };
        assert_eq!(setting_status_for(binding.clone(), &f), Ok(()));

        let binding = BackendBinding::GSettings {
            schema: "org.gnome.missing".into(),
            key: "x".into(),
        };
        assert!(
            setting_status_for(binding, &f)
                .unwrap_err()
                .contains("not present")
        );

        let binding = BackendBinding::GSettings {
            schema: "org.gnome.desktop.interface".into(),
            key: "no-such-key".into(),
        };
        assert!(
            setting_status_for(binding, &f)
                .unwrap_err()
                .contains("declares no key")
        );
    }

    #[test]
    fn missing_dbus_service_is_a_safe_unavailable_state() {
        let f = facts();
        let binding = BackendBinding::DBus {
            service: "org.freedesktop.portal.Desktop".into(),
            path: "/org/freedesktop/portal/desktop".into(),
            interface: "org.freedesktop.portal.Settings".into(),
            property: None,
        };
        assert_eq!(setting_status_for(binding, &f), Ok(()));
        let binding = BackendBinding::DBus {
            service: "org.example.Absent".into(),
            path: "/".into(),
            interface: "org.example.X".into(),
            property: None,
        };
        assert!(
            setting_status_for(binding, &f)
                .unwrap_err()
                .contains("not found on the session or system bus")
        );
    }

    fn setting_status_for(binding: BackendBinding, facts: &EnvironmentFacts) -> Result<(), String> {
        let def = module(Compatibility::default());
        let mut setting = def.settings[0].clone();
        setting.backend = binding;
        setting.availability = None;
        setting_status(&def, &setting, Some(facts))
    }

    #[test]
    fn facts_extraction_normalizes_app_ids_and_filters_backends() {
        let mut snapshot = snapshot(&facts());
        snapshot.installed_apps = vec![app("Alacritty.desktop")];
        snapshot.available_backends = vec![
            BackendCapability {
                id: "dbus-session".into(),
                kind: BackendKind::Service,
                status: BackendStatus::Available,
                evidence: vec![],
                detail: None,
            },
            BackendCapability {
                id: "config:zed".into(),
                kind: BackendKind::AppConfig,
                status: BackendStatus::Unavailable {
                    reason: "absent".into(),
                },
                evidence: vec![],
                detail: None,
            },
        ];
        let f = EnvironmentFacts::from_snapshot(&snapshot);
        assert!(f.installed_app_ids.contains("alacritty"));
        assert!(f.backend_ids.contains("dbus-session"));
        assert!(
            !f.backend_ids.contains("config:zed"),
            "unavailable capability excluded"
        );
    }
}
