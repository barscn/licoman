//! Merging discovered applications with registered application modules
//! (03_ENVIRONMENT_DISCOVERY.md §3, registry-driven since Phase 04).
//!
//! "A detected app is not automatically supported." The five conditions stay
//! separate:
//!
//! - **installed** — a desktop entry was discovered (this module's input);
//! - **module available** — a registered Licoman module matches the app;
//! - **app version compatible** — *not claimed* (modules that declare a
//!   version range are conservatively inapplicable, see `modules::compat`);
//! - **backend available** — the anticipated config location/backend probe
//!   for the app is available;
//! - **writable** — *not claimed in this phase* (Phase 06+).
//!
//! Matching is deterministic and explainable: desktop-file id equality
//! (case-insensitive against the module's declared `apps`, suffix-stripped)
//! or `StartupWMClass` equality as fallback. Nothing else matches — no fuzzy
//! heuristics. Since Phase 04 the module side comes from the module registry
//! (`AppModuleRef`), not a hand-written catalog.

use super::apps::InstalledApp;
use super::backends::BackendCapability;
use super::backends::BackendKind;
use crate::modules::compat::normalize_app_id;
use crate::modules::def::SupportLevel;
use crate::modules::registry::AppModuleRef;

/// One row of the merged Applications-window list.
#[derive(Debug, Clone, PartialEq)]
pub struct AppListEntry {
    /// Stable identity: the desktop-file id (e.g. `Alacritty.desktop`) or,
    /// for module-only rows, the module id.
    pub identity: String,
    /// The discovered application, when installed.
    pub app: Option<InstalledApp>,
    /// The registered module id, when a module matches.
    pub module: Option<String>,
    /// Module coverage: the module's declared support level when a module
    /// matches; [`SupportLevel::Unsupported`] only when no module matches —
    /// honestly shown as "no Licoman integration available yet".
    pub support: SupportLevel,
    /// The backend/config-location probe for this app is available.
    pub backend_available: bool,
    /// Display name: the discovered app name, else the module name.
    pub display_name: String,
}

impl AppListEntry {
    /// Primary visibility for the default "Supported" view: a module
    /// matches, the app is not hidden from menus and not restricted away
    /// from the current desktop.
    pub fn visible_as_supported(&self) -> bool {
        self.module.is_some()
            && self
                .app
                .as_ref()
                .map(|app| !app.no_display && !app.restricted_from_current)
                .unwrap_or(true)
    }
}

/// Pure merge over discovered apps, backend probes and registered
/// application modules. Deterministic: output is sorted by (module presence
/// first, then display name, then identity).
pub fn build_app_list(
    apps: &[InstalledApp],
    backends: &[BackendCapability],
    modules: &[AppModuleRef],
) -> Vec<AppListEntry> {
    let module_for = |candidate: &str| -> Option<&AppModuleRef> {
        let needle = normalize_app_id(candidate);
        modules
            .iter()
            .find(|&module| {
                module
                    .apps
                    .iter()
                    .any(|app| normalize_app_id(app) == needle)
            })
            .map(|v| v as _)
    };

    let backend_available = |app_id: &str| -> bool {
        let needle = app_id.to_ascii_lowercase();
        backends.iter().any(|backend| {
            backend.kind == BackendKind::AppConfig
                && backend.is_available()
                && backend
                    .id
                    .strip_prefix("config:")
                    .is_some_and(|id| needle.contains(id) || id == needle)
        })
    };

    let mut entries: Vec<AppListEntry> = Vec::new();
    let mut consumed_modules: Vec<&str> = Vec::new();

    for app in apps {
        // Module match: desktop-file id first, then StartupWMClass.
        let module_match =
            module_for(&app.id).or_else(|| app.wm_class.as_deref().and_then(module_for));

        if let Some(module) = module_match {
            consumed_modules.push(&module.module_id);
            entries.push(AppListEntry {
                identity: app.id.clone(),
                app: Some(app.clone()),
                module: Some(module.module_id.clone()),
                support: module.level,
                backend_available: backend_available(&app.id),
                display_name: app.name.clone(),
            });
        } else {
            entries.push(AppListEntry {
                identity: app.id.clone(),
                app: Some(app.clone()),
                module: None,
                support: SupportLevel::Unsupported,
                backend_available: backend_available(&app.id),
                display_name: app.name.clone(),
            });
        }
    }

    // Modules with no installed app still appear (module available, app not
    // installed) so the supported view works on machines without the app —
    // clearly labeled by the UI as "not installed".
    for module in modules {
        if !consumed_modules.contains(&module.module_id.as_str()) {
            entries.push(AppListEntry {
                identity: module.module_id.clone(),
                app: None,
                module: Some(module.module_id.clone()),
                support: module.level,
                backend_available: false,
                display_name: module.name.clone(),
            });
        }
    }

    entries.sort_by(|a, b| {
        let a_key = (
            a.module.is_none(),
            a.display_name.to_lowercase(),
            a.identity.clone(),
        );
        let b_key = (
            b.module.is_none(),
            b.display_name.to_lowercase(),
            b.identity.clone(),
        );
        a_key.cmp(&b_key)
    });
    entries
}

/// The exact message for unsupported apps (03_ENVIRONMENT_DISCOVERY.md UI
/// details): honest, no fake settings.
pub const NO_INTEGRATION_MESSAGE: &str = "No Licoman integration available yet.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::apps::InstallOrigin;

    fn app(id: &str, name: &str) -> InstalledApp {
        InstalledApp {
            id: id.to_string(),
            name: name.to_string(),
            generic_name: None,
            comment: None,
            icon: None,
            origin: InstallOrigin::System,
            source_path: format!("/usr/share/applications/{id}").into(),
            no_display: false,
            try_exec_found: true,
            restricted_from_current: false,
            wm_class: None,
            flatpak_id: None,
            evidence: Vec::new(),
        }
    }

    fn module(id: &str, name: &str, level: SupportLevel, apps: &[&str]) -> AppModuleRef {
        AppModuleRef {
            module_id: id.to_string(),
            name: name.to_string(),
            level,
            apps: apps.iter().map(|s| s.to_string()).collect(),
            provenance: Vec::new(),
        }
    }

    fn catalog() -> Vec<AppModuleRef> {
        vec![
            module(
                "app.alacritty",
                "Alacritty",
                SupportLevel::Supported,
                &["alacritty.desktop"],
            ),
            module(
                "app.kitty",
                "Kitty",
                SupportLevel::Supported,
                &["kitty.desktop"],
            ),
            module("app.mpv", "MPV", SupportLevel::Partial, &["mpv.desktop"]),
            module(
                "app.zed",
                "Zed",
                SupportLevel::ReadOnly,
                &["dev.zed.Zed.desktop"],
            ),
        ]
    }

    #[test]
    fn matches_by_desktop_file_id() {
        let apps = vec![app("alacritty.desktop", "Alacritty")];
        let list = build_app_list(&apps, &[], &catalog());
        let entry = list
            .iter()
            .find(|e| e.identity == "alacritty.desktop")
            .unwrap();
        assert_eq!(entry.module.as_deref(), Some("app.alacritty"));
        assert_eq!(entry.support, SupportLevel::Supported);
        assert!(entry.visible_as_supported());
    }

    #[test]
    fn matches_by_startup_wm_class_as_fallback() {
        let mut entry_app = app("org.unknown.Terminal.desktop", "Terminal");
        entry_app.wm_class = Some("kitty".to_string());
        let list = build_app_list(&[entry_app], &[], &catalog());
        let entry = list
            .iter()
            .find(|e| e.identity == "org.unknown.Terminal.desktop")
            .unwrap();
        assert_eq!(entry.module.as_deref(), Some("app.kitty"));
    }

    #[test]
    fn unknown_app_is_installed_but_unsupported() {
        let apps = vec![app("org.flatpak.HeavyApp.desktop", "Heavy App")];
        let list = build_app_list(&apps, &[], &catalog());
        let entry = list
            .iter()
            .find(|e| e.identity == "org.flatpak.HeavyApp.desktop")
            .unwrap();
        assert!(entry.app.is_some(), "installed");
        assert_eq!(entry.module, None, "no module");
        assert_eq!(entry.support, SupportLevel::Unsupported);
        assert!(!entry.visible_as_supported());
    }

    #[test]
    fn uninstalled_modules_still_list_with_module_available() {
        let list = build_app_list(&[], &[], &catalog());
        let zed = list.iter().find(|e| e.identity == "app.zed").unwrap();
        assert!(zed.app.is_none());
        assert_eq!(zed.module.as_deref(), Some("app.zed"));
        assert_eq!(zed.support, SupportLevel::ReadOnly);
    }

    #[test]
    fn no_display_and_restricted_apps_leave_the_supported_view() {
        let mut hidden = app("alacritty.desktop", "Alacritty");
        hidden.no_display = true;
        let mut restricted = app("kitty.desktop", "Kitty");
        restricted.restricted_from_current = true;
        let list = build_app_list(&[hidden, restricted], &[], &catalog());
        for entry in list.iter().filter(|e| e.app.is_some()) {
            assert!(
                !entry.visible_as_supported(),
                "{} should not be primary",
                entry.identity
            );
        }
        // Module-only rows (app not installed) legitimately stay visible:
        // the supported view is driven by registered modules.
        assert!(
            list.iter()
                .any(|e| e.app.is_none() && e.visible_as_supported())
        );
    }

    #[test]
    fn backend_availability_is_a_separate_condition() {
        let apps = vec![app("alacritty.desktop", "Alacritty")];
        let backend = BackendCapability {
            id: "config:alacritty".to_string(),
            kind: BackendKind::AppConfig,
            status: super::super::backends::BackendStatus::Available,
            evidence: Vec::new(),
            detail: None,
        };
        let list = build_app_list(&apps, &[backend], &catalog());
        let entry = list
            .iter()
            .find(|e| e.identity == "alacritty.desktop")
            .unwrap();
        assert!(entry.backend_available);
    }

    #[test]
    fn list_is_deterministically_sorted() {
        let apps = vec![
            app("zzz.desktop", "Zeta"),
            app("alacritty.desktop", "Alacritty"),
        ];
        let list = build_app_list(&apps, &[], &catalog());
        let names: Vec<&str> = list.iter().map(|e| e.display_name.as_str()).collect();
        // Module-backed entries first, then by name.
        assert_eq!(names[0], "Alacritty");
        assert_eq!(names[1], "Kitty");
    }
}
