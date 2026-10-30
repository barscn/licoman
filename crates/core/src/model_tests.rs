//! Lifecycle tests for the store (02_FOUNDATION.md tests §1): load → edit →
//! discard; load → edit → mock apply; invalid value; mock failure; conflict.

use super::*;
use crate::descriptor::{ApplyMode, Control, Scope, SettingDescriptor};
use crate::error::ErrorKind;
use crate::mock::MockSettingFixture;
use crate::prefs::{LoadDiagnostic, Preferences};
use crate::provider::{Capabilities, ProbeResult, Provider};
use crate::value::SettingType;

fn single_store(fixture: MockSettingFixture) -> Store {
    let mut registry = Registry::new();
    registry.register(Box::new(crate::mock::MockProvider::new(
        "test",
        vec![fixture],
    )));
    store_from(registry)
}

fn store_from(registry: Registry) -> Store {
    Store::new(
        registry,
        LoadedPreferences {
            preferences: Preferences::default(),
            diagnostics: Vec::new(),
        },
        std::env::temp_dir().join("licoman-model-test-prefs.toml"),
    )
}

fn staged_bool(id: &'static str, initial: bool) -> MockSettingFixture {
    MockSettingFixture::new(
        SettingDescriptor::new(
            id,
            Scope::Shared,
            "C",
            "X",
            "",
            SettingType::Bool,
            Control::Switch,
        ),
        SettingValue::Bool(initial),
    )
}

fn current_phase(store: &Store, id: &str) -> SettingPhase {
    store.model(id).expect("model").phase.clone()
}

// ---- load → edit → discard -------------------------------------------------

#[test]
fn load_then_edit_then_discard_returns_to_stored_value() {
    let mut store = single_store(staged_bool("t.x", true));
    assert_eq!(current_phase(&store, "t.x"), SettingPhase::Idle);
    assert_eq!(store.effective_value("t.x"), Some(SettingValue::Bool(true)));

    let report = store.dispatch(Command::Edit {
        id: "t.x".into(),
        value: SettingValue::Bool(false),
    });
    assert!(report.settings_changed);
    assert_eq!(current_phase(&store, "t.x"), SettingPhase::Dirty);
    assert_eq!(store.pending_count(), 1);
    // The stored value is untouched — editing is not persisting.
    assert_eq!(
        store.model("t.x").unwrap().stored,
        Some(SettingValue::Bool(true))
    );

    store.dispatch(Command::DiscardAll);
    assert_eq!(current_phase(&store, "t.x"), SettingPhase::Idle);
    assert_eq!(store.pending_count(), 0);
    assert_eq!(store.effective_value("t.x"), Some(SettingValue::Bool(true)));
}

// ---- load → edit → mock apply (with readback) ------------------------------

#[test]
fn load_edit_apply_flow_reaches_applied_with_confirmed_readback() {
    let mut store = single_store(staged_bool("t.x", false));
    store.dispatch(Command::Edit {
        id: "t.x".into(),
        value: SettingValue::Bool(true),
    });
    store.dispatch(Command::ApplyAll);
    assert!(store.applying());
    assert_eq!(current_phase(&store, "t.x"), SettingPhase::Applying);

    let report = store.apply_complete();
    assert_eq!(report.outcome, Outcome::Applied(vec!["t.x".to_string()]));
    let model = store.model("t.x").unwrap();
    assert_eq!(model.phase, SettingPhase::Applied);
    assert_eq!(model.readback, Readback::Confirmed);
    assert_eq!(model.stored, Some(SettingValue::Bool(true)));
    assert_eq!(model.edited, None);
    assert!(!store.applying());

    // The transient confirmation returns to Idle without changing the value.
    assert!(store.acknowledge_applied());
    assert_eq!(current_phase(&store, "t.x"), SettingPhase::Idle);
    assert_eq!(store.effective_value("t.x"), Some(SettingValue::Bool(true)));
}

// ---- invalid value ----------------------------------------------------------

#[test]
fn invalid_value_is_rejected_at_apply_with_user_safe_error() {
    let fixture = MockSettingFixture::new(
        {
            let mut d = SettingDescriptor::new(
                "t.n",
                Scope::Shared,
                "C",
                "Number",
                "",
                SettingType::Integer,
                Control::Number { step: 1., unit: "" },
            );
            d.min = Some(0.);
            d.max = Some(64.);
            d
        },
        SettingValue::Integer(5),
    );
    let mut store = single_store(fixture);
    // The out-of-range edit is accepted as a dirty edit (the control clamps
    // in practice; the store does not guess).
    store.dispatch(Command::Edit {
        id: "t.n".into(),
        value: SettingValue::Integer(65),
    });
    store.dispatch(Command::ApplyAll);
    let model = store.model("t.n").unwrap();
    assert_eq!(model.phase, SettingPhase::Failed);
    let error = model.error.as_ref().expect("validation error recorded");
    assert_eq!(error.kind, ErrorKind::Validation);
    assert_eq!(error.user_message, "Number must be between 0 and 64.");
    assert!(!error.technical.is_empty());
    // Nothing was written: the stored value still reads 5.
    assert_eq!(model.stored, Some(SettingValue::Integer(5)));

    // Revert drops the failed edit and returns to the stored value.
    store.dispatch(Command::Revert { id: "t.n".into() });
    assert_eq!(current_phase(&store, "t.n"), SettingPhase::Idle);
    assert_eq!(store.effective_value("t.n"), Some(SettingValue::Integer(5)));
}

// ---- mock failure ------------------------------------------------------------

#[test]
fn mock_write_failure_lands_in_failed_with_retry_and_revert() {
    let mut store = single_store(staged_bool("t.x", false).fail_writes());
    store.dispatch(Command::Edit {
        id: "t.x".into(),
        value: SettingValue::Bool(true),
    });
    store.dispatch(Command::ApplyAll);
    store.apply_complete();
    let model = store.model("t.x").unwrap();
    assert_eq!(model.phase, SettingPhase::Failed);
    assert_eq!(model.error.as_ref().unwrap().kind, ErrorKind::WriteFailed);
    assert_eq!(
        model.error.as_ref().unwrap().user_message,
        "The mock provider rejected this write."
    );

    // Retry runs the same failing write and stays Failed.
    store.dispatch(Command::Retry { id: "t.x".into() });
    assert_eq!(current_phase(&store, "t.x"), SettingPhase::Failed);

    // Revert gives up on the edit.
    store.dispatch(Command::Revert { id: "t.x".into() });
    assert_eq!(current_phase(&store, "t.x"), SettingPhase::Idle);
    assert_eq!(
        store.effective_value("t.x"),
        Some(SettingValue::Bool(false))
    );
}

// ---- external conflict → reconcile -------------------------------------------

#[test]
fn source_change_between_reads_is_reported_as_external_conflict() {
    // The accent fixture reads #70D0A9 first and #8AB4FF on the verify pass,
    // so a plain load already produces the conflict.
    let mut registry = Registry::new();
    registry.register(Box::new(crate::mock::MockProvider::standard()));
    let store = store_from(registry);
    let model = store.model("desktop.session.accent").unwrap();
    assert_eq!(model.phase, SettingPhase::ExternalConflict);
    assert_eq!(model.stored, Some(SettingValue::Choice("#70D0A9".into())));
    assert_eq!(model.external, Some(SettingValue::Choice("#8AB4FF".into())));
    assert!(model.phase.blocks_editing());

    // Reload reconciles by accepting the source value.
    let mut store = store;
    let report = store.dispatch(Command::Reload {
        id: "desktop.session.accent".into(),
    });
    assert!(report.settings_changed);
    let model = store.model("desktop.session.accent").unwrap();
    assert_eq!(model.phase, SettingPhase::Idle);
    assert_eq!(model.stored, Some(SettingValue::Choice("#8AB4FF".into())));
    assert_eq!(model.external, None);
}

#[test]
fn readback_mismatch_is_surfaced_as_conflict_not_success() {
    // A provider whose read disagrees with what it just accepted.
    struct LyingProvider;
    impl Provider for LyingProvider {
        fn id(&self) -> &'static str {
            "liar"
        }
        fn probe(&self) -> ProbeResult {
            ProbeResult::available(Capabilities {
                read: true,
                write: true,
                watch: false,
            })
        }
        fn descriptors(&self) -> Vec<SettingDescriptor> {
            vec![SettingDescriptor::new(
                "t.x",
                Scope::Shared,
                "C",
                "X",
                "",
                SettingType::Bool,
                Control::Switch,
            )]
        }
        fn read(&self, _id: &str) -> Result<SettingValue, LicomanError> {
            // Always reports false, even after a write of true.
            Ok(SettingValue::Bool(false))
        }
        fn write(&self, _id: &str, _value: &SettingValue) -> Result<(), LicomanError> {
            Ok(())
        }
    }
    let mut registry = Registry::new();
    registry.register(Box::new(LyingProvider));
    let mut store = store_from(registry);
    store.dispatch(Command::Edit {
        id: "t.x".into(),
        value: SettingValue::Bool(true),
    });
    store.dispatch(Command::ApplyAll);
    store.apply_complete();
    let model = store.model("t.x").unwrap();
    assert_eq!(model.phase, SettingPhase::ExternalConflict);
    assert_eq!(model.readback, Readback::Mismatch);
}

// ---- immediate vs staged apply metadata ---------------------------------------

#[test]
fn immediate_mode_applies_synchronously_on_edit() {
    let fixture = MockSettingFixture::new(
        {
            let mut d = SettingDescriptor::new(
                "t.i",
                Scope::Shared,
                "C",
                "Immediate",
                "",
                SettingType::Bool,
                Control::Switch,
            );
            d.apply_mode = ApplyMode::Immediate;
            d
        },
        SettingValue::Bool(false),
    );
    let mut store = single_store(fixture);
    let report = store.dispatch(Command::Edit {
        id: "t.i".into(),
        value: SettingValue::Bool(true),
    });
    assert_eq!(report.outcome, Outcome::Applied(vec!["t.i".to_string()]));
    let model = store.model("t.i").unwrap();
    assert_eq!(model.phase, SettingPhase::Applied);
    assert_eq!(model.readback, Readback::Confirmed);
    assert!(!store.applying());
}

#[test]
fn immediate_mode_failure_is_reported_without_touching_the_source() {
    let fixture = MockSettingFixture::new(
        {
            let mut d = SettingDescriptor::new(
                "t.i",
                Scope::Shared,
                "C",
                "Immediate",
                "",
                SettingType::Bool,
                Control::Switch,
            );
            d.apply_mode = ApplyMode::Immediate;
            d
        },
        SettingValue::Bool(false),
    )
    .fail_writes();
    let mut store = single_store(fixture);
    let report = store.dispatch(Command::Edit {
        id: "t.i".into(),
        value: SettingValue::Bool(true),
    });
    assert!(matches!(report.outcome, Outcome::Applied(_)));
    let model = store.model("t.i").unwrap();
    assert_eq!(model.phase, SettingPhase::Failed);
    assert_eq!(model.error.as_ref().unwrap().kind, ErrorKind::WriteFailed);
    // The failed edit is retained for Retry/Revert.
    assert_eq!(model.edited, Some(SettingValue::Bool(true)));
}

// ---- edit guards ---------------------------------------------------------------

#[test]
fn read_only_and_conflicting_settings_refuse_edits() {
    let mut registry = Registry::new();
    registry.register(Box::new(crate::mock::MockProvider::standard()));
    let mut store = store_from(registry);
    // Read-only control.
    let report = store.dispatch(Command::Edit {
        id: "distro.services.timesync".into(),
        value: SettingValue::Text("other".into()),
    });
    assert_eq!(report.outcome, Outcome::NoOp);
    // External conflict blocks editing.
    let report = store.dispatch(Command::Edit {
        id: "desktop.session.accent".into(),
        value: SettingValue::Choice("#F2C36C".into()),
    });
    assert_eq!(report.outcome, Outcome::NoOp);
}

// ---- unavailable settings --------------------------------------------------------

#[test]
fn unavailable_settings_load_as_unavailable_with_user_safe_reason() {
    let mut registry = Registry::new();
    registry.register(Box::new(crate::mock::MockProvider::standard()));
    let store = store_from(registry);
    let model = store.model("shared.power.profile").unwrap();
    assert_eq!(model.phase, SettingPhase::Unavailable);
    assert_eq!(model.stored, None);
    let error = model.error.as_ref().unwrap();
    assert_eq!(error.kind, ErrorKind::Unavailable);
    assert_eq!(
        error.user_message,
        "No power-profiles-daemon detected in this mock session."
    );
}

#[test]
fn permission_held_settings_are_unavailable_with_permission_kind() {
    let mut registry = Registry::new();
    registry.register(Box::new(crate::mock::MockProvider::standard()));
    let store = store_from(registry);
    let model = store.model("distro.packages.updates").unwrap();
    assert_eq!(model.phase, SettingPhase::Unavailable);
    assert_eq!(model.error.as_ref().unwrap().kind, ErrorKind::Permission);
}

// ---- two views observe the same effective data -------------------------------------

#[test]
fn two_window_views_observe_the_same_effective_data() {
    let mut registry = Registry::new();
    registry.register(Box::new(crate::mock::MockProvider::standard()));
    let mut store = store_from(registry);
    // One window edits Wi-Fi through the command path…
    store.dispatch(Command::Edit {
        id: "shared.network.wifi".into(),
        value: SettingValue::Bool(false),
    });
    // …the other window reads through a completely different access path
    // (search hit → effective value). Both must agree.
    let hits = store.search("wi-fi");
    assert_eq!(hits.len(), 1);
    let from_search = hits[0].id.clone();
    assert_eq!(
        store.effective_value(&from_search),
        Some(SettingValue::Bool(false))
    );
    assert_eq!(
        store.model("shared.network.wifi").unwrap().effective(),
        Some(&SettingValue::Bool(false))
    );
    // And a scope selection change does not alter the data either view sees.
    store.dispatch(Command::SelectScope(Scope::Desktop));
    assert_eq!(
        store.effective_value("shared.network.wifi"),
        Some(SettingValue::Bool(false))
    );
}

// ---- no modules gracefully -----------------------------------------------------------

#[test]
fn empty_registry_is_usable_and_reports_no_settings() {
    let mut store = store_from(Registry::new());
    assert!(store.models().is_empty());
    assert!(store.search("anything").is_empty());
    assert_eq!(store.pending_count(), 0);
    assert!(store.descriptors_in_scope(Scope::Shared).is_empty());
    assert_eq!(store.categories(Scope::Desktop), Vec::<&str>::new());
    let report = store.dispatch(Command::ApplyAll);
    assert_eq!(report.outcome, Outcome::NoOp);
}

// ---- selections are independent ---------------------------------------------------------

#[test]
fn window_selections_are_independent() {
    let mut registry = Registry::new();
    registry.register(Box::new(crate::mock::MockProvider::standard()));
    let mut store = store_from(registry);
    store.dispatch(Command::SelectApp { index: Some(2) });
    store.dispatch(Command::SelectAppTab(1));
    store.dispatch(Command::SelectScope(Scope::Distribution));
    assert_eq!(store.selected_app(), Some(2));
    assert_eq!(store.selected_app_tab(), 1);
    assert_eq!(store.selected_scope(), Scope::Distribution);
    // Theme changes and scope changes do not clobber apps-window selection.
    store.dispatch(Command::SelectScope(Scope::Shared));
    assert_eq!(store.selected_app(), Some(2));
    assert_eq!(store.selected_app_tab(), 1);
}

// ---- Licoman theme preference: immediate, separate from provider edits -----

#[test]
fn theme_preference_is_an_immediate_licoman_only_write() {
    let mut registry = Registry::new();
    registry.register(Box::new(crate::mock::MockProvider::standard()));
    let path = std::env::temp_dir().join(format!("licoman-theme-test-{}.toml", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut store = Store::new(
        registry,
        LoadedPreferences {
            preferences: Preferences::default(),
            diagnostics: vec![LoadDiagnostic::Corrupt],
        },
        path.clone(),
    );
    assert_eq!(store.preferences().theme, AppearanceMode::Dark);

    let report = store.dispatch(Command::SetAppearance(AppearanceMode::Light));
    assert!(report.preferences_changed);
    assert!(!report.settings_changed, "theme is not a provider setting");
    assert_eq!(store.preferences().theme, AppearanceMode::Light);
    // The safe immediate write landed in Licoman's own file.
    let reloaded = crate::prefs::load_from(&path);
    assert_eq!(reloaded.preferences.theme, AppearanceMode::Light);
    let _ = std::fs::remove_file(&path);
}

// ---- demo hooks -----------------------------------------------------------------

#[test]
fn demo_hooks_set_conflict_and_failed_states() {
    let mut registry = Registry::new();
    registry.register(Box::new(crate::mock::MockProvider::standard()));
    let mut store = store_from(registry);
    store.simulate_external_change("shared.network.wifi", SettingValue::Bool(false));
    assert_eq!(
        current_phase(&store, "shared.network.wifi"),
        SettingPhase::ExternalConflict
    );
    store.simulate_failed("shared.network.dns");
    assert_eq!(
        current_phase(&store, "shared.network.dns"),
        SettingPhase::Failed
    );
}

// ---- restart metadata -------------------------------------------------------------

#[test]
fn applied_restart_target_setting_sets_restart_required() {
    let mut registry = Registry::new();
    registry.register(Box::new(crate::mock::MockProvider::standard()));
    let mut store = store_from(registry);
    store.dispatch(Command::Edit {
        id: "desktop.session.keyboard_repeat_delay".into(),
        value: SettingValue::Choice("500 ms".into()),
    });
    store.dispatch(Command::ApplyAll);
    store.apply_complete();
    let model = store
        .model("desktop.session.keyboard_repeat_delay")
        .unwrap();
    assert_eq!(model.phase, SettingPhase::Applied);
    assert!(model.restart_required);
}
