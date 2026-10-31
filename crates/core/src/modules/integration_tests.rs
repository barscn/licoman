//! Integration-level tests for the module registry feeding the store
//! (04_MODULE_SCHEMA_AND_REGISTRY.md §7 test matrix, Phase 04): high-volume
//! synthetic catalogs, app removal mid-session, edits flowing through the
//! store over manifest-defined settings, and navigation preservation across
//! registry refreshes.

use std::sync::{Arc, Mutex};

use super::ModuleProvider;
use super::registry::{ModuleRegistry, ModuleSource};
use crate::command::Outcome;
use crate::descriptor::Scope;
use crate::discovery::demo_fixtures;
use crate::prefs::{LoadedPreferences, Preferences};
use crate::value::SettingValue;
use crate::{Command, Registry, Store};

fn volume_module(id: &str, start: usize, count: usize) -> String {
    let mut text = String::new();
    text.push_str(&format!(
        "schema_version = 1\nid = \"{id}\"\nscope = \"shared\"\ntitle = \"Volume {id}\"\ncategory = \"Synthetic\"\n\n"
    ));
    for i in start..start + count {
        text.push_str(&format!(
            "[[settings]]\nid = \"setting{i}\"\ntitle = \"Synthetic setting {i:04}\"\n\
             description = \"Generated volume fixture row {i}.\"\nkeywords = [\"synthetic\"]\n\
             type = \"boolean\"\ncontrol = \"switch\"\ndefault = {}\n\n[settings.backend]\nkind = \"mock\"\n\n",
            i % 2 == 0
        ));
    }
    text
}

fn provider_for(texts: Vec<(&'static str, String)>) -> ModuleProvider {
    let registry = ModuleRegistry::load(
        texts
            .into_iter()
            .map(|(name, text)| (ModuleSource::Builtin(name), text)),
    );
    assert!(
        registry.rejected().is_empty(),
        "fixtures must validate: {:#?}",
        registry
            .rejected()
            .iter()
            .flat_map(|r| &r.issues)
            .take(3)
            .collect::<Vec<_>>()
    );
    let provider = ModuleProvider::new(Arc::new(Mutex::new(registry)));
    provider.set_environment(Some(&demo_fixtures::empty_environment()));
    provider
}

fn store_with(provider: &ModuleProvider) -> Store {
    let mut registry = Registry::new();
    registry.register(Box::new(ModuleProvider::clone(provider)));
    let mut store = Store::new(
        registry,
        LoadedPreferences {
            preferences: Preferences::default(),
            diagnostics: Vec::new(),
        },
        std::env::temp_dir().join("licoman-module-volume-test-prefs.toml"),
    );
    store.reload_all();
    store
}

/// 1,000+ mock settings: search and navigation stay responsive and every
/// identity stays stable (04 §7 "1,000+ mock settings").
#[test]
fn high_volume_catalog_search_and_navigation() {
    let started = std::time::Instant::now();
    // Three modules of 400 (the per-module limit is 512): 1,200 settings.
    let provider = provider_for(vec![
        ("v1", volume_module("shared.volume.a", 0, 400)),
        ("v2", volume_module("shared.volume.b", 400, 400)),
        ("v3", volume_module("shared.volume.c", 800, 400)),
    ]);
    let store = store_with(&provider);

    let descriptors = store.registry().descriptors();
    assert_eq!(descriptors.len(), 1_200);
    // Stable, unique identities across the whole catalog.
    let ids: std::collections::BTreeSet<&str> = descriptors.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(ids.len(), 1_200);

    // Scope/category grouping still works at this volume.
    let categories = store.categories(Scope::Shared);
    assert_eq!(categories, vec!["Synthetic".to_string()]);
    assert_eq!(store.scope_rows(Scope::Shared).len(), 1_200);

    // Search over the precomputed index (not a per-keystroke rebuild).
    let hits = store.search("setting 0777");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "shared.volume.b.setting777");
    assert_eq!(store.search("synthetic").len(), 1_200);
    assert!(store.search("nothing-matches-this").is_empty());

    // Navigation by identity works after the volume load.
    assert!(store.descriptor("shared.volume.a.setting0").is_some());
    assert!(store.descriptor("shared.volume.c.setting1199").is_some());

    let elapsed = started.elapsed();
    crate::log::info(
        "core::modules::tests",
        "volume test finished",
        &[("millis", elapsed.as_millis().to_string())],
    );
    // Generous bound: the point is "no pathological cost", not a benchmark.
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "volume catalog took too long: {elapsed:?}"
    );
}

/// High-volume edits: staged edit and apply through manifest-backed settings.
#[test]
fn volume_catalog_edits_flow_through_the_store() {
    let provider = provider_for(vec![("v", volume_module("shared.volume.a", 0, 400))]);
    let mut store = store_with(&provider);
    let id = "shared.volume.a.setting0".to_string();

    store.dispatch(Command::Edit {
        id: id.clone(),
        value: SettingValue::Bool(false),
    });
    assert_eq!(store.pending_count(), 1);
    store.dispatch(Command::ApplyAll);
    let report = store.apply_complete();
    assert!(matches!(report.outcome, Outcome::Applied(ref applied) if applied.contains(&id)));
    assert_eq!(
        store.model(&id).expect("model").phase,
        crate::state::SettingPhase::Applied
    );
}

/// Unavailable app removed during the session: applicability flips to
/// inapplicable with a preserved reason, the store reloads gracefully and
/// navigation/edits survive where identities still exist
/// (04 §7 "Unavailable app removed during session", §4 "Handle removed
/// apps/modules gracefully").
#[test]
fn app_removed_mid_session_is_graceful() {
    const APP_MODULE: &str = r#"
schema_version = 1
id = "app.removable"
scope = "application"
title = "Removable"
category = "Test"

[compatibility]
apps = ["removable.desktop"]

[[settings]]
id = "toggle"
title = "Toggle"
type = "boolean"
control = "switch"
default = true

[settings.backend]
kind = "mock"
"#;
    let registry = ModuleRegistry::load([(ModuleSource::Builtin("a"), APP_MODULE.to_string())]);
    assert!(registry.rejected().is_empty());
    let provider = ModuleProvider::new(Arc::new(Mutex::new(registry)));

    let mut with_app = demo_fixtures::empty_environment();
    with_app
        .installed_apps
        .push(crate::discovery::InstalledApp {
            id: "removable.desktop".to_string(),
            name: "Removable".to_string(),
            generic_name: None,
            comment: None,
            icon: None,
            origin: crate::discovery::InstallOrigin::System,
            source_path: std::path::PathBuf::from("/usr/share/applications/removable.desktop"),
            no_display: false,
            try_exec_found: true,
            restricted_from_current: false,
            wm_class: None,
            flatpak_id: None,
            evidence: Vec::new(),
        });

    // Installed: applicable, mock read serves the declared default.
    assert!(provider.set_environment(Some(&with_app)));
    let mut store = store_with(&provider);
    let id = "app.removable.toggle";
    assert!(store.descriptor(id).is_some());
    assert_eq!(
        store.model(id).expect("model").phase,
        crate::state::SettingPhase::Idle
    );

    // Stage an edit, then the app disappears from the environment.
    store.dispatch(Command::Edit {
        id: id.to_string(),
        value: SettingValue::Bool(false),
    });
    assert_eq!(store.pending_count(), 1);

    let without_app = demo_fixtures::empty_environment();
    assert!(
        provider.set_environment(Some(&without_app)),
        "availability changed"
    );
    store.reload_all();

    // Truthful state, no fake values, no panic.
    let model = store.model(id).expect("model still registered");
    assert_eq!(model.phase, crate::state::SettingPhase::Unavailable);
    assert!(
        model
            .error
            .as_ref()
            .expect("reason preserved")
            .user_message
            .contains("none of them is installed")
    );
    assert_eq!(
        store.pending_count(),
        0,
        "staged edit on a now-unavailable row dropped"
    );
}

/// Navigation and staged edits survive a registry refresh when identities
/// still exist (04 §6 "Preserve App/Setting navigation when a module is
/// refreshed").
#[test]
fn reload_preserves_navigation_and_edits_for_surviving_ids() {
    let provider = provider_for(vec![("v", volume_module("shared.volume.a", 0, 50))]);
    let mut store = store_with(&provider);

    store.dispatch(Command::SelectScope(Scope::Desktop));
    let id = "shared.volume.a.setting3".to_string();
    store.dispatch(Command::Edit {
        id: id.clone(),
        value: SettingValue::Bool(true),
    });

    // Refresh with the same environment: nothing is lost.
    store.reload_all();
    assert_eq!(store.selected_scope(), Scope::Desktop);
    assert_eq!(store.pending_count(), 1);
    assert_eq!(
        store.effective_value(&id),
        Some(SettingValue::Bool(true)),
        "staged edit survives the refresh"
    );
}

/// Immediate-policy mock settings apply on edit, completed by the apply
/// pass (synchronous here; background in the UI — no unverified "Applied").
#[test]
fn immediate_policy_applies_on_edit() {
    const IMMEDIATE: &str = r#"
schema_version = 1
id = "shared.immediate.demo"
scope = "shared"
title = "Immediate"
category = "Test"

[[settings]]
id = "toggle"
title = "Toggle"
type = "boolean"
control = "switch"
policy = "immediate"
default = false

[settings.backend]
kind = "mock"
"#;
    let provider = provider_for(vec![("i", IMMEDIATE.to_string())]);
    let mut store = store_with(&provider);
    let report = store.dispatch(Command::Edit {
        id: "shared.immediate.demo.toggle".to_string(),
        value: SettingValue::Bool(true),
    });
    assert!(matches!(report.outcome, Outcome::Accepted));
    let report = store.apply_complete();
    assert!(matches!(report.outcome, Outcome::Applied(_)));
    let model = store.model("shared.immediate.demo.toggle").expect("model");
    assert_eq!(model.phase, crate::state::SettingPhase::Applied);
    assert_eq!(model.readback, crate::state::Readback::Confirmed);
}

/// Real-backend rows never become editable through any store path: the
/// provider refuses reads and writes (hard boundary, 04 work item 7).
#[test]
fn real_backend_settings_never_activate_writes() {
    const REAL: &str = r#"
schema_version = 1
id = "desktop.gnome.example"
scope = "desktop"
title = "GNOME Example"
category = "Test"

[compatibility]
required_backends = ["gsettings"]

[[settings]]
id = "clock.show_weekday"
title = "Show weekday"
type = "boolean"
control = "switch"
default = true

[settings.backend]
kind = "gsettings"
schema = "org.gnome.desktop.interface"
key = "clock-show-weekday"
"#;
    let registry = ModuleRegistry::load([(ModuleSource::Builtin("g"), REAL.to_string())]);
    // Reads go through a fixture router (synthetic schema + dconf fixtures)
    // so the test never depends on the host's installed schemas.
    let fixtures =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let router = crate::read::BackendRouter::new(
        crate::read::gsettings::GSettingsReadBackend::for_paths(
            vec![fixtures.join("discovery/schemas")],
            Some(fixtures.join("gsettings/dconf/user")),
            vec![fixtures.join("gsettings/dconf/system/site")],
        ),
        crate::read::file::FileReadBackend::for_dirs(
            fixtures.join("file-config"),
            fixtures.join("file-home"),
            vec![],
        ),
        crate::read::dbus::DBusReadBackend::with_client(Arc::new(AbsentService)),
    );
    let provider = ModuleProvider::with_router(Arc::new(Mutex::new(registry)), router);
    // The Fedora/GNOME fixture carries the gsettings capability AND the
    // schema facts, so module compatibility and the binding presence check
    // pass and the read reaches the Phase 05 backend (fixture-backed).
    provider.set_environment(Some(&demo_fixtures::fedora_gnome_wayland()));

    let mut store = store_with(&provider);
    let id = "desktop.gnome.example.clock.show_weekday";
    // Background-latency settings start Unknown; the adapter reads them off
    // the UI thread and publishes.
    assert_eq!(
        store.model(id).expect("model").phase,
        crate::state::SettingPhase::Unknown
    );
    let read = crate::provider::Provider::read_detailed(&provider, id);
    assert!(store.publish_read(id, read));
    let model = store.model(id).expect("model");
    // The fixture dconf user db sets the key to true: a real read.
    assert_eq!(model.phase, crate::state::SettingPhase::Idle);
    assert_eq!(model.stored, Some(SettingValue::Bool(true)));
    assert_eq!(
        model.read_meta.as_ref().expect("provenance").origin,
        crate::read::ValueOrigin::User
    );
    // Writes are still refused unconditionally (Phase 06 owns writing).
    let error =
        crate::provider::Provider::write(&provider, id, &SettingValue::Bool(false)).unwrap_err();
    assert_eq!(error.kind, crate::error::ErrorKind::WriteFailed);
    assert!(error.user_message.contains("Nothing was modified"));
    // Editing is blocked at the store level (read-only envelope) and
    // refused at the provider level.
    let report = store.dispatch(Command::Edit {
        id: id.to_string(),
        value: SettingValue::Bool(false),
    });
    assert_eq!(report.outcome, Outcome::NoOp);
}

/// D-Bus client stub: no PowerProfiles service in these tests.
struct AbsentService;

impl crate::read::dbus::PowerProfilesClient for AbsentService {
    fn locate(
        &self,
    ) -> Result<Option<(&'static str, &'static str, &'static str)>, crate::read::dbus::ClientError>
    {
        Ok(None)
    }
    fn property(
        &self,
        _endpoint: (&'static str, &'static str, &'static str),
        _name: &str,
    ) -> Result<crate::read::dbus::PropertyValue, crate::read::dbus::ClientError> {
        Err(crate::read::dbus::ClientError::Other("absent".into()))
    }
}
