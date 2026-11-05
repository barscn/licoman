//! The store: loaded setting models, dirty tracking, validation, staged and
//! immediate apply, revert/reconcile semantics, window selections and
//! Licoman's own preferences. All of it is UI-independent and synchronous;
//! the only asynchrony (a visible "Applying…" interval) is driven by the
//! adapter splitting a staged apply into begin/complete calls.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::command::{Command, DispatchReport, Outcome};
use crate::descriptor::{ApplyMode, Control, Scope, SettingDescriptor};
use crate::error::{ErrorKind, LicomanError};
use crate::prefs::{AppearanceMode, LoadedPreferences};
use crate::registry::Registry;
use crate::search::{SearchHit, SearchIndex};
use crate::state::{Readback, SettingPhase};
use crate::value::{self, SettingValue};
use crate::write::{Change, ChangeIntent, Persistence, WritePolicy, derive_write_policy};

/// Bounded undo history (06_SAFE_WRITE_ENGINE.md §5: undo only while the
/// original state is known; the history is deliberately small).
pub const MAX_UNDO_ENTRIES: usize = 32;

/// One reversible, applied change: the store can restore `before` through
/// the same safe write path while `after` is still the source value.
#[derive(Debug, Clone, PartialEq)]
pub struct UndoEntry {
    pub setting_id: String,
    pub before: SettingValue,
    pub after: SettingValue,
}

/// Observable state of one setting, kept by the store. Fields are read-only
/// to the UI; mutations happen through [`Store::dispatch`] only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingModel {
    /// Last value confirmed from the source.
    pub stored: Option<SettingValue>,
    /// Value seen at the source during an external conflict.
    pub external: Option<SettingValue>,
    /// The user's staged edit, if any.
    pub edited: Option<SettingValue>,
    pub phase: SettingPhase,
    pub readback: Readback,
    /// Provenance of the last successful read (origin, freshness, source
    /// details) — the Source details disclosure renders this.
    pub read_meta: Option<crate::read::ReadResult>,
    /// Set when an apply revealed a restart requirement (descriptor metadata
    /// declares it up front; this flag records it per apply).
    pub restart_required: bool,
    /// How the last successful apply persists, as far as the backend knows
    /// (06_SAFE_WRITE_ENGINE.md §3: runtime_only / persistent / unknown,
    /// never claimed durable without evidence).
    pub persistence: Option<Persistence>,
    pub error: Option<LicomanError>,
}

impl SettingModel {
    /// The value the UI should present: the staged edit if present, else the
    /// stored value.
    pub fn effective(&self) -> Option<&SettingValue> {
        self.edited.as_ref().or(self.stored.as_ref())
    }
}

pub struct Store {
    registry: Registry,
    models: BTreeMap<String, SettingModel>,
    /// Precomputed normalized search text (04_MODULE_SCHEMA_AND_REGISTRY.md
    /// §6): rebuilt with load/reload, never per keystroke.
    search_index: SearchIndex,
    // Main-window selections.
    selected_scope: Scope,
    selected_category: Option<String>,
    // Applications-window selections (independent of the main window).
    selected_app: Option<String>,
    selected_app_tab: usize,
    // Licoman-only preferences — the single user-owned file Licoman writes.
    preferences: crate::prefs::Preferences,
    preference_diagnostics: Vec<crate::prefs::LoadDiagnostic>,
    preferences_path: PathBuf,
    /// True between apply-begin and apply-complete.
    applying: bool,
    /// Change intents staged for rows currently in the Applying phase; the
    /// adapter drains them for background execution, tests drain them
    /// through the synchronous apply path.
    pending_intents: HashMap<String, ChangeIntent>,
    /// Staged changes whose derived policy is Confirmation: they stay
    /// Pending until the user explicitly confirms in the dialog
    /// (06_SAFE_WRITE_ENGINE.md §1).
    awaiting_confirmation: Vec<String>,
    /// Bounded undo history of applied changes (newest last).
    undo_history: Vec<UndoEntry>,
}

impl Store {
    /// Build a store over `registry` and load every setting from its provider.
    /// `preferences` comes from [`crate::prefs::load_from`] so startup I/O
    /// stays in one place.
    pub fn new(
        registry: Registry,
        preferences: LoadedPreferences,
        preferences_path: PathBuf,
    ) -> Self {
        let mut store = Self {
            registry,
            models: BTreeMap::new(),
            search_index: SearchIndex::default(),
            // The Summary panel is the main window's landing view.
            selected_scope: Scope::Summary,
            selected_category: None,
            selected_app: None,
            selected_app_tab: 0,
            preferences: preferences.preferences,
            preference_diagnostics: preferences.diagnostics,
            preferences_path,
            applying: false,
            pending_intents: HashMap::new(),
            awaiting_confirmation: Vec::new(),
            undo_history: Vec::new(),
        };
        store.load();
        crate::log::info(
            "core::store",
            "settings loaded",
            &[
                ("providers", store.registry.providers().len().to_string()),
                ("settings", store.models.len().to_string()),
            ],
        );
        store
    }

    /// Read every registered setting from its provider, then run a
    /// verification pass whose mismatch is reported as an external conflict.
    ///
    /// Background-latency settings (real I/O backends — files, GSettings,
    /// D-Bus) are **not** read here: they stay in `Unknown` and the adapter
    /// schedules a background read pass, publishing through
    /// [`Store::publish_read`]. Nothing on the UI thread blocks on I/O
    /// (05_READ_ONLY_BACKENDS.md §2: "Failure of one backend must not …
    /// block the UI thread").
    pub fn load(&mut self) {
        self.models.clear();
        self.search_index = SearchIndex::build(self.registry.descriptors());
        let descriptors: Vec<SettingDescriptor> = self.registry.descriptors().to_vec();
        for descriptor in &descriptors {
            let model = if self.registry.read_latency(&descriptor.id)
                == crate::provider::ReadLatency::Background
            {
                SettingModel {
                    phase: SettingPhase::Unknown,
                    ..SettingModel::default()
                }
            } else {
                match self.registry.read_detailed(&descriptor.id) {
                    Ok(result) => SettingModel {
                        stored: Some(result.value.clone()),
                        read_meta: Some(result),
                        phase: SettingPhase::Idle,
                        ..SettingModel::default()
                    },
                    Err(error) => SettingModel {
                        phase: SettingPhase::Unavailable,
                        error: Some(error),
                        ..SettingModel::default()
                    },
                }
            };
            self.models.insert(descriptor.id.to_string(), model);
        }
        self.verify_against_source();
    }

    /// Re-read every setting after the registry content changed (module
    /// applicability recomputed on an environment refresh). Staged edits on
    /// ids that still exist and remain editable are restored, and the
    /// search index is rebuilt; window selections live in separate fields
    /// and survive untouched (04_MODULE_SCHEMA_AND_REGISTRY.md §6).
    pub fn reload_all(&mut self) {
        let edits: Vec<(String, SettingValue)> = self
            .models
            .iter()
            .filter(|(id, m)| {
                m.edited.is_some()
                    && self
                        .registry
                        .descriptor(id)
                        .is_some_and(|d| d.control != Control::ReadOnly)
            })
            .map(|(id, m)| (id.clone(), m.edited.clone().expect("filtered above")))
            .collect();
        let had_edits = !edits.is_empty();
        self.load();
        for (id, value) in edits {
            let Some(model) = self.models.get_mut(&id) else {
                continue;
            };
            if model.phase.blocks_editing() {
                continue; // the setting became unavailable; drop the edit
            }
            model.edited = Some(value);
            model.phase = SettingPhase::Dirty;
        }
        if had_edits {
            crate::log::info(
                "core::store",
                "reload preserved staged edits",
                &[("count", self.pending_count().to_string())],
            );
        }
    }

    /// Re-read readable settings; a differing value means the source changed
    /// outside Licoman since the first read (the stand-in for the watch that
    /// arrives with real backends — no polling, no daemon).
    fn verify_against_source(&mut self) {
        let ids: Vec<String> = self
            .models
            .iter()
            .filter(|(_, m)| m.phase == SettingPhase::Idle)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let current = self.registry.read(&id);
            let model = self.models.get_mut(&id).expect("id came from models");
            if let Ok(value) = current
                && model.stored.as_ref() != Some(&value)
            {
                model.external = Some(value);
                model.phase = SettingPhase::ExternalConflict;
            }
        }
    }

    // ---- read-only accessors ----------------------------------------------

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Ids whose values come from background (real I/O) backends. The
    /// adapter reads these off the UI thread and publishes the results.
    pub fn background_read_ids(&self) -> Vec<String> {
        self.models
            .iter()
            .filter(|(id, _)| {
                self.registry.read_latency(id) == crate::provider::ReadLatency::Background
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Publish the result of one background read. Staged edits survive
    /// (the read only refreshes the stored value and provenance); a value
    /// plus fingerprint identical to the previous read emits no change.
    /// Returns whether the UI should be notified.
    pub fn publish_read(
        &mut self,
        id: &str,
        result: Result<crate::read::ReadResult, LicomanError>,
    ) -> bool {
        use crate::read::ReadResult as RR;
        let Some(model) = self.models.get_mut(id) else {
            return false;
        };
        match result {
            Ok(result) => {
                let unchanged = match (&model.read_meta, model.stored.as_ref()) {
                    (Some(previous), Some(stored)) => {
                        previous.freshness.fingerprint == result.freshness.fingerprint
                            && previous.origin == result.origin
                            && stored.equals(&result.value)
                    }
                    _ => false,
                };
                model.stored = Some(result.value.clone());
                model.read_meta = Some(result);
                model.error = None;
                if !model.phase.holds_pending_edit()
                    && model.phase != SettingPhase::ExternalConflict
                {
                    model.phase = SettingPhase::Idle;
                }
                // A stashed retry intent follows the fresh revision so a
                // retry re-checks the current source, not a stale read.
                if let Some(intent) = self.pending_intents.get_mut(id) {
                    intent.expected_fingerprint = model
                        .read_meta
                        .as_ref()
                        .map(|meta| meta.freshness.fingerprint.clone());
                    intent.before = model.stored.clone();
                }
                !unchanged
            }
            Err(error) => {
                // A failed background read is a truthful unavailable state —
                // unless a previous read succeeded (keep the last known value
                // and surface the error in the details instead of blanking).
                match model.read_meta {
                    Some(RR { .. }) if model.stored.is_some() => {
                        model.error = Some(error);
                        false
                    }
                    _ => {
                        model.phase = SettingPhase::Unavailable;
                        model.error = Some(error);
                        true
                    }
                }
            }
        }
    }

    /// Re-run every background read's availability verdict through the
    /// provider (used when the environment changed): rows whose modules
    /// became inapplicable get their reason without waiting for the read
    /// pass. Implemented as a plain reload trigger by the adapter.
    pub fn background_rows_pending(&self) -> bool {
        self.models
            .values()
            .any(|m| m.phase == SettingPhase::Unknown)
    }

    pub fn descriptor(&self, id: &str) -> Option<&SettingDescriptor> {
        self.registry.descriptor(id)
    }

    pub fn model(&self, id: &str) -> Option<&SettingModel> {
        self.models.get(id)
    }

    pub fn models(&self) -> &BTreeMap<String, SettingModel> {
        &self.models
    }

    pub fn descriptors_in_scope(&self, scope: Scope) -> Vec<&SettingDescriptor> {
        self.registry.descriptors_in_scope(scope)
    }

    /// Owned snapshot of (descriptor, model) pairs for one scope, in display
    /// order. The view layer renders from snapshots like this one and never
    /// holds domain borrows.
    pub fn scope_rows(&self, scope: Scope) -> Vec<(SettingDescriptor, SettingModel)> {
        self.registry
            .descriptors_in_scope(scope)
            .into_iter()
            .map(|descriptor| {
                let model = self.models.get(&descriptor.id).cloned().unwrap_or_default();
                (descriptor.clone(), model)
            })
            .collect()
    }

    /// Categories within a scope, in registration order.
    pub fn categories(&self, scope: Scope) -> Vec<String> {
        let mut categories: Vec<String> = Vec::new();
        for descriptor in self.registry.descriptors_in_scope(scope) {
            if !categories.contains(&descriptor.category) {
                categories.push(descriptor.category.clone());
            }
        }
        categories
    }

    /// Owned (descriptor, model) snapshots for an explicit id list (used by
    /// the Applications window for one module's settings). Ids that are not
    /// registered are skipped; order follows the input list.
    pub fn rows_for_ids(&self, ids: &[String]) -> Vec<(SettingDescriptor, SettingModel)> {
        ids.iter()
            .filter_map(|id| {
                let descriptor = self.registry.descriptor(id)?;
                let model = self.models.get(id).cloned().unwrap_or_default();
                Some((descriptor.clone(), model))
            })
            .collect()
    }

    pub fn effective_value(&self, id: &str) -> Option<SettingValue> {
        self.models.get(id).and_then(|m| m.effective().cloned())
    }

    pub fn selected_scope(&self) -> Scope {
        self.selected_scope
    }

    pub fn selected_category(&self) -> Option<&str> {
        self.selected_category.as_deref()
    }

    pub fn selected_app(&self) -> Option<&str> {
        self.selected_app.as_deref()
    }

    pub fn selected_app_tab(&self) -> usize {
        self.selected_app_tab
    }

    pub fn preferences(&self) -> &crate::prefs::Preferences {
        &self.preferences
    }

    pub fn preference_diagnostics(&self) -> &[crate::prefs::LoadDiagnostic] {
        &self.preference_diagnostics
    }

    pub fn preferences_path(&self) -> &PathBuf {
        &self.preferences_path
    }

    pub fn applying(&self) -> bool {
        self.applying
    }

    /// The core-derived write policy for one setting (None = unregistered).
    /// Modules cannot weaken it: derivation lives in
    /// [`crate::write::derive_write_policy`] (06_SAFE_WRITE_ENGINE.md §1).
    pub fn write_policy(&self, id: &str) -> Option<WritePolicy> {
        let descriptor = self.descriptor(id)?;
        Some(derive_write_policy(
            descriptor,
            self.registry.write_risk(id),
        ))
    }

    /// Staged changes waiting for the user's explicit confirmation dialog.
    pub fn confirmation_pending(&self) -> &[String] {
        &self.awaiting_confirmation
    }

    /// Whether the last applied change for `id` can be undone right now:
    /// the original value is known, the source still holds the applied
    /// value, and the row is idle or freshly applied
    /// (06_SAFE_WRITE_ENGINE.md §5).
    pub fn undo_available(&self, id: &str) -> bool {
        let Some(entry) = self.undo_history.iter().rev().find(|e| e.setting_id == id) else {
            return false;
        };
        matches!(
            self.model(id).map(|m| m.phase.clone()),
            Some(SettingPhase::Idle) | Some(SettingPhase::Applied)
        ) && self.model(id).and_then(|m| m.stored.as_ref()) == Some(&entry.after)
    }

    /// Reset support for one setting, routed through the owning provider.
    pub fn reset_support(&self, id: &str) -> Option<crate::write::ResetSupport> {
        self.registry.reset_support(id)
    }

    /// Rows holding an unapplied edit (the apply bar's "N changes pending").
    pub fn pending_count(&self) -> usize {
        self.models
            .values()
            .filter(|m| {
                m.phase.holds_pending_edit()
                    || (m.phase == SettingPhase::Failed && m.edited.is_some())
            })
            .count()
    }

    /// Ids of rows holding an unapplied edit, sorted by stable id.
    pub fn pending_ids(&self) -> Vec<String> {
        self.models
            .iter()
            .filter(|(_, m)| {
                m.phase.holds_pending_edit()
                    || (m.phase == SettingPhase::Failed && m.edited.is_some())
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn search(&self, query: &str) -> Vec<SearchHit> {
        self.search_index.search(query)
    }

    // ---- command dispatch -------------------------------------------------

    pub fn dispatch(&mut self, command: Command) -> DispatchReport {
        match command {
            Command::Edit { id, value } => self.edit(&id, value),
            Command::ApplyAll => self.apply_begin(),
            Command::DiscardAll => self.discard_all(),
            Command::Discard { id } => self.discard(&id),
            Command::Retry { id } => self.retry(&id),
            Command::Revert { id } => self.revert(&id),
            Command::Reload { id } => self.reload(&id),
            Command::Undo { id } => self.undo(&id),
            Command::ResetToDefault { id } => self.reset_to_default(&id),
            Command::ConfirmApply { id } => self.confirm_apply(&id),
            Command::SelectScope(scope) => {
                self.selected_scope = scope;
                self.selected_category = None;
                DispatchReport {
                    outcome: Outcome::Accepted,
                    settings_changed: false,
                    preferences_changed: false,
                }
            }
            Command::SelectCategory(category) => {
                self.selected_category = category;
                DispatchReport {
                    outcome: Outcome::Accepted,
                    settings_changed: false,
                    preferences_changed: false,
                }
            }
            Command::SelectApp { id } => {
                self.selected_app = id;
                DispatchReport {
                    outcome: Outcome::Accepted,
                    settings_changed: false,
                    preferences_changed: false,
                }
            }
            Command::SelectAppTab(tab) => {
                self.selected_app_tab = tab;
                DispatchReport {
                    outcome: Outcome::Accepted,
                    settings_changed: false,
                    preferences_changed: false,
                }
            }
            Command::SetAppearance(mode) => self.set_appearance(mode),
        }
    }

    fn edit(&mut self, id: &str, value: SettingValue) -> DispatchReport {
        // Guards run without holding a model borrow across registry calls.
        let Some(descriptor) = self.registry.descriptor(id).cloned() else {
            return DispatchReport::noop();
        };
        if descriptor.control == Control::ReadOnly {
            return DispatchReport::noop();
        }
        let policy = self.write_policy(id);
        {
            let Some(model) = self.models.get(id) else {
                return DispatchReport::noop();
            };
            if model.phase.blocks_editing() {
                return DispatchReport::noop();
            }
            if model.effective() == Some(&value) {
                return DispatchReport::noop();
            }
        }

        // Permission architecture (06_SAFE_WRITE_ENGINE.md §4): a setting
        // behind an authorization boundary never stages. The row shows the
        // distinct Permission-required state with the honest explanation.
        if policy == Some(WritePolicy::Privileged) {
            let scope = descriptor
                .permission_scope
                .clone()
                .unwrap_or_else(|| "an authorization boundary".to_string());
            let error = LicomanError::new(
                ErrorKind::Permission,
                format!(
                    "Requires authorization — {scope}. Licoman runs unprivileged and \
                     privileged writes are denied by default in this build. Nothing \
                     was modified."
                ),
                format!("deny-by-default privilege broker refused {id:?}"),
            );
            let model = self.models.get_mut(id).expect("models cover descriptors");
            model.phase = SettingPhase::PermissionRequired;
            model.error = Some(error.clone());
            return DispatchReport {
                outcome: Outcome::Rejected(error),
                settings_changed: true,
                preferences_changed: false,
            };
        }

        if descriptor.apply_mode == ApplyMode::Immediate {
            return self.edit_immediate(id, &descriptor, value);
        }

        // Staged (and Confirmation — confirmation happens at Apply): a
        // control change is only a dirty flag, never persistence.
        let model = self.models.get_mut(id).expect("models cover descriptors");
        model.error = None;
        model.readback = Readback::NotYet;
        model.edited = Some(value);
        model.phase = SettingPhase::Dirty;
        crate::log::info("core::store", "staged edit", &[("id", id.to_string())]);
        DispatchReport {
            outcome: Outcome::Accepted,
            settings_changed: true,
            preferences_changed: false,
        }
    }

    /// Immediate mode: validate, stage the write intent, and let the apply
    /// pass execute it (in tests: the synchronous apply; in the UI: the
    /// background pass, so no I/O ever runs on the UI thread). A failure
    /// lands the row in Failed with a user-safe message and the edit
    /// retained for Retry/Revert.
    fn edit_immediate(
        &mut self,
        id: &str,
        descriptor: &SettingDescriptor,
        value: SettingValue,
    ) -> DispatchReport {
        if let Err(error) = value::validate(descriptor, &value) {
            let model = self.models.get_mut(id).expect("models cover descriptors");
            model.phase = SettingPhase::Failed;
            model.error = Some(error.clone());
            return DispatchReport {
                outcome: Outcome::Rejected(error),
                settings_changed: true,
                preferences_changed: false,
            };
        }
        let intent = self.intent_for_edited(id, Some(value.clone()));
        let Some(intent) = intent else {
            return DispatchReport::noop();
        };
        {
            let model = self.models.get_mut(id).expect("models cover descriptors");
            model.phase = SettingPhase::Applying;
            model.error = None;
            // Retained while applying and after a failure, so Retry/Revert
            // and the visible value behave exactly like a staged edit.
            model.edited = Some(value);
        }
        self.pending_intents.insert(id.to_string(), intent);
        self.applying = true;
        crate::log::info(
            "core::store",
            "immediate edit staged for apply",
            &[("id", id.to_string())],
        );
        DispatchReport {
            outcome: Outcome::Accepted,
            settings_changed: true,
            preferences_changed: false,
        }
    }

    /// Build the write intent for one setting from its model state:
    /// before-value from the last confirmed read, expected fingerprint from
    /// the read provenance.
    fn intent_for_edited(&self, id: &str, edited: Option<SettingValue>) -> Option<ChangeIntent> {
        let model = self.models.get(id)?;
        let change_value = edited.or_else(|| model.edited.clone())?;
        Some(ChangeIntent {
            setting_id: id.to_string(),
            before: model.stored.clone(),
            change: Change::Set(change_value),
            expected_fingerprint: model
                .read_meta
                .as_ref()
                .map(|meta| meta.freshness.fingerprint.clone()),
        })
    }

    /// Validate and stage every dirty edit, then mark the batch applying.
    /// Rows whose derived policy is Confirmation stay Pending and are listed
    /// for the confirmation dialog; everything else becomes an intent for
    /// the apply pass. The caller executes intents off the UI thread
    /// ([`Store::apply_complete`] synchronously, or the adapter's background
    /// pass).
    pub fn apply_begin(&mut self) -> DispatchReport {
        if self.applying {
            return DispatchReport::noop();
        }
        let ids = self.pending_ids();
        if ids.is_empty() {
            return DispatchReport::noop();
        }
        let mut rejected: Option<LicomanError> = None;
        self.awaiting_confirmation.clear();
        for id in &ids {
            let Some(descriptor) = self.registry.descriptor(id).cloned() else {
                continue;
            };
            let policy = self.write_policy(id);
            let Some(model) = self.models.get_mut(id) else {
                continue;
            };
            if model.phase == SettingPhase::Failed {
                continue; // retry/revert own failed rows
            }
            model.phase = SettingPhase::Validating;
            let Some(edited) = model.edited.clone() else {
                continue;
            };
            match value::validate(&descriptor, &edited) {
                Ok(()) => {
                    if policy == Some(WritePolicy::Confirmation) {
                        model.phase = SettingPhase::Pending;
                        self.awaiting_confirmation.push(id.clone());
                    } else {
                        model.phase = SettingPhase::Applying;
                    }
                }
                Err(error) => {
                    model.phase = SettingPhase::Failed;
                    model.error = Some(error.clone());
                    rejected.get_or_insert(error);
                }
            }
        }
        // Intents for everything now Applying (confirmation rows stay
        // Pending and are applied one explicit ConfirmApply at a time).
        let applying_ids: Vec<String> = self
            .models
            .iter()
            .filter(|(id, m)| {
                m.phase == SettingPhase::Applying && !self.pending_intents.contains_key(*id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in applying_ids {
            if let Some(intent) = self.intent_for_edited(&id, None) {
                self.pending_intents.insert(id, intent);
            }
        }
        let applying_now = self
            .models
            .values()
            .filter(|m| m.phase == SettingPhase::Applying)
            .count();
        self.applying = applying_now > 0;
        if self.awaiting_confirmation.is_empty() && applying_now == 0 {
            self.applying = false;
        }
        crate::log::info(
            "core::store",
            "apply begin",
            &[
                ("staged", ids.len().to_string()),
                ("applying", applying_now.to_string()),
                (
                    "awaiting_confirmation",
                    self.awaiting_confirmation.len().to_string(),
                ),
            ],
        );
        DispatchReport {
            outcome: match rejected {
                Some(error) => Outcome::Rejected(error),
                None => Outcome::Accepted,
            },
            settings_changed: true,
            preferences_changed: false,
        }
    }

    /// Drain the intents for rows currently in the Applying phase. Rows with
    /// neither an intent nor an edit cannot be written and fall back to
    /// Idle. The adapter runs these on the background executor; tests run
    /// them through [`Store::apply_complete`].
    pub fn take_apply_intents(&mut self) -> Vec<(String, ChangeIntent)> {
        let mut intents = Vec::new();
        let applying: Vec<String> = self
            .models
            .iter()
            .filter(|(_, m)| m.phase == SettingPhase::Applying)
            .map(|(id, _)| id.clone())
            .collect();
        for id in applying {
            if let Some(intent) = self.pending_intents.remove(&id) {
                intents.push((id, intent));
                continue;
            }
            if let Some(intent) = self.intent_for_edited(&id, None) {
                intents.push((id, intent));
            } else {
                // Applying without a value: nothing to write, honestly idle.
                if let Some(model) = self.models.get_mut(&id) {
                    model.phase = SettingPhase::Idle;
                }
            }
        }
        intents
    }

    /// The single mapping from a write attempt to observable phases. Used by
    /// the synchronous path and the adapter's background pass alike, so both
    /// can never disagree about what "Applied" means: only a verification
    /// read that matches the request earns it.
    pub fn publish_apply(
        &mut self,
        id: &str,
        intent: &ChangeIntent,
        result: Result<crate::provider::WriteOutcome, LicomanError>,
    ) -> bool {
        let Some(model) = self.models.get_mut(id) else {
            return false;
        };
        let new_value = intent.change.value().cloned();
        match result {
            Err(write_error) => {
                match write_error.kind {
                    ErrorKind::Conflict => {
                        // The source changed externally; the write was
                        // aborted before touching anything.
                        model.phase = SettingPhase::ExternalConflict;
                        model.error = Some(write_error);
                        model.readback = Readback::Mismatch;
                        // Keep the edit and the intent: the user may reload
                        // (which refreshes the revision) and retry.
                        self.pending_intents.insert(id.to_string(), intent.clone());
                    }
                    ErrorKind::Permission => {
                        model.phase = SettingPhase::PermissionRequired;
                        model.error = Some(write_error);
                        self.pending_intents.remove(id);
                    }
                    _ => {
                        model.phase = SettingPhase::Failed;
                        model.error = Some(write_error);
                        self.pending_intents.insert(id.to_string(), intent.clone());
                    }
                }
                true
            }
            Ok(write) => {
                let expect_absent =
                    matches!(intent.change, Change::RemoveOverride) && write.outcome.expect_absent;
                let verified = if expect_absent {
                    // Removal verified by the backend; a failed verification
                    // read ("not present") corroborates it.
                    write.outcome.verified
                } else {
                    match &write.verification {
                        Some(read) => new_value.as_ref() == Some(&read.value),
                        None => false, // no verification, no "Applied"
                    }
                };
                if verified {
                    if let Some(read) = write.verification {
                        model.stored = Some(read.value.clone());
                        model.read_meta = Some(read);
                    } else if expect_absent {
                        // The key is gone; no value exists at the source.
                        model.stored = None;
                    }
                    model.edited = None;
                    model.external = None;
                    model.phase = SettingPhase::Applied;
                    model.readback = Readback::Confirmed;
                    model.error = None;
                    model.persistence = Some(write.outcome.persistence);
                    model.restart_required = self
                        .registry
                        .descriptor(id)
                        .is_some_and(|d| d.restart_target.is_some())
                        || write.outcome.restart != crate::read::RestartEffect::None;
                    if let (Some(before), Some(after)) = (intent.before.clone(), new_value) {
                        self.push_undo(UndoEntry {
                            setting_id: id.to_string(),
                            before,
                            after,
                        });
                    }
                } else if let Some(read) = &write.verification {
                    // The write landed but the source holds something else:
                    // the source is authoritative, this is a conflict.
                    model.stored = new_value.clone();
                    model.read_meta = Some(read.clone());
                    model.edited = None;
                    model.external = read.value.clone().into();
                    model.phase = SettingPhase::ExternalConflict;
                    model.readback = Readback::Mismatch;
                    model.persistence = Some(write.outcome.persistence);
                    if let (Some(before), Some(after)) = (intent.before.clone(), new_value) {
                        self.push_undo(UndoEntry {
                            setting_id: id.to_string(),
                            before,
                            after,
                        });
                    }
                } else {
                    // Write landed (or may have) but verification failed:
                    // never a success badge.
                    model.phase = SettingPhase::Failed;
                    model.error = Some(write.verification_error.unwrap_or_else(|| {
                        LicomanError::new(
                            ErrorKind::WriteFailed,
                            "The change was written but could not be verified. \
                                 Refresh before making further changes.",
                            format!("unverified write for {id}"),
                        )
                    }));
                    if let Some(after) = new_value
                        && let Some(before) = intent.before.clone()
                    {
                        self.push_undo(UndoEntry {
                            setting_id: id.to_string(),
                            before,
                            after,
                        });
                    }
                }
                true
            }
        };
        if !self
            .models
            .values()
            .any(|m| m.phase == SettingPhase::Applying)
        {
            self.applying = false;
        }
        crate::log::info(
            "core::store",
            "apply published",
            &[
                ("id", id.to_string()),
                (
                    "phase",
                    self.models
                        .get(id)
                        .map(|m| format!("{:?}", m.phase))
                        .unwrap_or_default(),
                ),
            ],
        );
        true
    }

    fn push_undo(&mut self, entry: UndoEntry) {
        self.undo_history
            .retain(|existing| existing.setting_id != entry.setting_id);
        self.undo_history.push(entry);
        while self.undo_history.len() > MAX_UNDO_ENTRIES {
            self.undo_history.remove(0);
        }
    }

    /// Synchronous apply for tests and mock-only flows: executes every
    /// Applying row's intent through the registry's safe write path and
    /// publishes the results. The UI adapter uses the same machinery on the
    /// background executor instead (no I/O on the UI thread).
    pub fn apply_complete(&mut self) -> DispatchReport {
        if !self.applying {
            return DispatchReport::noop();
        }
        let intents = self.take_apply_intents();
        let mut applied = Vec::new();
        for (id, intent) in intents {
            let result = self.registry.write_intent(&intent);
            self.publish_apply(&id, &intent, result);
            if self
                .models
                .get(&id)
                .is_some_and(|m| m.phase == SettingPhase::Applied)
            {
                applied.push(id);
            }
        }
        self.applying = false;
        crate::log::info(
            "core::store",
            "apply complete",
            &[("applied", applied.len().to_string())],
        );
        DispatchReport {
            outcome: if applied.is_empty() {
                Outcome::NoOp
            } else {
                Outcome::Applied(applied)
            },
            settings_changed: true,
            preferences_changed: false,
        }
    }

    fn discard_all(&mut self) -> DispatchReport {
        let mut changed = false;
        for model in self.models.values_mut() {
            if model.phase.holds_pending_edit() {
                model.edited = None;
                model.phase = SettingPhase::Idle;
                model.error = None;
                changed = true;
            }
        }
        DispatchReport {
            outcome: if changed {
                Outcome::Accepted
            } else {
                Outcome::NoOp
            },
            settings_changed: changed,
            preferences_changed: false,
        }
    }

    fn discard(&mut self, id: &str) -> DispatchReport {
        let Some(model) = self.models.get_mut(id) else {
            return DispatchReport::noop();
        };
        if model.phase.holds_pending_edit() {
            model.edited = None;
            model.phase = SettingPhase::Idle;
            model.error = None;
            DispatchReport {
                outcome: Outcome::Accepted,
                settings_changed: true,
                preferences_changed: false,
            }
        } else {
            DispatchReport::noop()
        }
    }

    fn retry(&mut self, id: &str) -> DispatchReport {
        let Some(descriptor) = self.registry.descriptor(id).cloned() else {
            return DispatchReport::noop();
        };
        let (phase_is_failed, edited) = {
            let Some(model) = self.models.get(id) else {
                return DispatchReport::noop();
            };
            (model.phase == SettingPhase::Failed, model.edited.clone())
        };
        if !phase_is_failed {
            return DispatchReport::noop();
        }
        let Some(edited) = edited else {
            return DispatchReport::noop();
        };
        if let Err(error) = value::validate(&descriptor, &edited) {
            let model = self.models.get_mut(id).expect("checked above");
            model.error = Some(error.clone());
            return DispatchReport {
                outcome: Outcome::Rejected(error),
                settings_changed: true,
                preferences_changed: false,
            };
        }
        if !self.pending_intents.contains_key(id)
            && let Some(intent) = self.intent_for_edited(id, Some(edited))
        {
            self.pending_intents.insert(id.to_string(), intent);
        }
        let model = self.models.get_mut(id).expect("checked above");
        model.phase = SettingPhase::Applying;
        self.applying = true;
        DispatchReport {
            outcome: Outcome::Accepted,
            settings_changed: true,
            preferences_changed: false,
        }
    }

    fn revert(&mut self, id: &str) -> DispatchReport {
        let Some(model) = self.models.get_mut(id) else {
            return DispatchReport::noop();
        };
        if model.phase == SettingPhase::Failed || model.phase == SettingPhase::PermissionRequired {
            model.edited = None;
            model.error = None;
            model.phase = SettingPhase::Idle;
            self.pending_intents.remove(id);
            DispatchReport {
                outcome: Outcome::Accepted,
                settings_changed: true,
                preferences_changed: false,
            }
        } else {
            DispatchReport::noop()
        }
    }

    /// Re-read from the source. For an external conflict this reconciles by
    /// accepting the source value (the source stays authoritative,
    /// 00_START_HERE.md non-negotiable 1). The read provenance is refreshed
    /// too, so a follow-up intent (retry, undo, reset) checks the current
    /// revision. A stashed retry intent is refreshed so a retry after
    /// reload re-checks the new revision instead of false-conflicting on
    /// the stale one.
    fn reload(&mut self, id: &str) -> DispatchReport {
        if self.model(id).is_some_and(|m| m.phase.holds_pending_edit()) {
            return DispatchReport::noop();
        }
        if self.model(id).is_none() {
            return DispatchReport::noop();
        }
        match self.registry.read_detailed(id) {
            Ok(result) => {
                let model = self.models.get_mut(id).expect("checked above");
                model.stored = Some(result.value.clone());
                model.read_meta = Some(result);
                model.external = None;
                model.error = None;
                model.phase = SettingPhase::Idle;
                let fresh = model
                    .read_meta
                    .as_ref()
                    .map(|meta| meta.freshness.fingerprint.clone());
                if let Some(intent) = self.pending_intents.get_mut(id) {
                    intent.expected_fingerprint = fresh;
                    intent.before = model.stored.clone();
                }
                DispatchReport {
                    outcome: Outcome::Accepted,
                    settings_changed: true,
                    preferences_changed: false,
                }
            }
            Err(error) => {
                let model = self.models.get_mut(id).expect("checked above");
                model.phase = SettingPhase::Unavailable;
                model.error = Some(error.clone());
                DispatchReport {
                    outcome: Outcome::Rejected(error),
                    settings_changed: true,
                    preferences_changed: false,
                }
            }
        }
    }

    /// Undo the last applied change: re-apply the recorded before-value
    /// through the same safe write path. The engine re-checks the source
    /// first, so undoing on top of an external change conflicts instead of
    /// clobbering (06_SAFE_WRITE_ENGINE.md §5). Failures are reportable:
    /// the row lands in Failed with the error.
    fn undo(&mut self, id: &str) -> DispatchReport {
        let Some(entry_position) = self
            .undo_history
            .iter()
            .rposition(|entry| entry.setting_id == id)
        else {
            return DispatchReport::noop();
        };
        if !self.undo_available(id) {
            return DispatchReport::noop();
        }
        let entry = self.undo_history.remove(entry_position);
        let Some(descriptor) = self.registry.descriptor(id).cloned() else {
            return DispatchReport::noop();
        };
        if descriptor.control == Control::ReadOnly {
            self.push_undo(entry);
            return DispatchReport::noop();
        }
        if let Err(error) = value::validate(&descriptor, &entry.before) {
            // Restore the history entry: the undo was refused, not lost.
            self.push_undo(entry);
            return DispatchReport {
                outcome: Outcome::Rejected(error),
                settings_changed: true,
                preferences_changed: false,
            };
        }
        let intent = ChangeIntent {
            setting_id: id.to_string(),
            before: Some(entry.after.clone()),
            change: Change::Set(entry.before.clone()),
            expected_fingerprint: self
                .models
                .get(id)
                .and_then(|m| m.read_meta.as_ref())
                .map(|meta| meta.freshness.fingerprint.clone()),
        };
        {
            let model = self.models.get_mut(id).expect("checked above");
            model.phase = SettingPhase::Applying;
            model.error = None;
        }
        self.pending_intents.insert(id.to_string(), intent);
        self.applying = true;
        crate::log::info("core::store", "undo staged", &[("id", id.to_string())]);
        DispatchReport {
            outcome: Outcome::Accepted,
            settings_changed: true,
            preferences_changed: false,
        }
    }

    /// Reset one setting to its default. The concrete semantics come from
    /// the backend and were disclosed to the user before this command
    /// (06_SAFE_WRITE_ENGINE.md §5): deleting a user override, or writing a
    /// literal declared default.
    fn reset_to_default(&mut self, id: &str) -> DispatchReport {
        let Some(descriptor) = self.registry.descriptor(id).cloned() else {
            return DispatchReport::noop();
        };
        let phase_ok = self
            .model(id)
            .is_some_and(|m| matches!(m.phase, SettingPhase::Idle | SettingPhase::Applied));
        if !phase_ok || descriptor.control == Control::ReadOnly {
            return DispatchReport::noop();
        }
        let policy = self.write_policy(id);
        if matches!(
            policy,
            Some(WritePolicy::ReadOnly) | Some(WritePolicy::Privileged)
        ) {
            return DispatchReport::noop();
        }
        let Some(support) = self.registry.reset_support(id) else {
            return DispatchReport::noop();
        };
        let (change, expected_absent_note) = match &support.disclosure {
            crate::write::ResetDisclosure::NotPossible { reason } => {
                let error = LicomanError::new(
                    ErrorKind::Validation,
                    format!("Reset is not available for this setting: {reason}"),
                    format!("reset unsupported for {id}"),
                );
                return DispatchReport {
                    outcome: Outcome::Rejected(error),
                    settings_changed: false,
                    preferences_changed: false,
                };
            }
            crate::write::ResetDisclosure::WritesLiteral { .. } => {
                let Some(default_value) = support.default_value.clone() else {
                    return DispatchReport::noop();
                };
                (Change::Set(default_value), false)
            }
            crate::write::ResetDisclosure::DeletesOverride { .. } => (Change::RemoveOverride, true),
        };
        let _ = expected_absent_note;
        let intent = ChangeIntent {
            setting_id: id.to_string(),
            before: self.models.get(id).and_then(|m| m.stored.clone()),
            change,
            expected_fingerprint: self
                .models
                .get(id)
                .and_then(|m| m.read_meta.as_ref())
                .map(|meta| meta.freshness.fingerprint.clone()),
        };
        {
            let model = self.models.get_mut(id).expect("checked above");
            model.phase = SettingPhase::Applying;
            model.error = None;
        }
        self.pending_intents.insert(id.to_string(), intent);
        self.applying = true;
        crate::log::info("core::store", "reset staged", &[("id", id.to_string())]);
        DispatchReport {
            outcome: Outcome::Accepted,
            settings_changed: true,
            preferences_changed: false,
        }
    }

    /// The user confirmed one staged change whose policy demanded explicit
    /// confirmation. Only rows actually listed as awaiting confirmation may
    /// proceed (06_SAFE_WRITE_ENGINE.md §1: the Confirmation policy is
    /// enforced by the core, not by the dialog).
    fn confirm_apply(&mut self, id: &str) -> DispatchReport {
        if !self
            .awaiting_confirmation
            .iter()
            .any(|pending| pending == id)
        {
            return DispatchReport::noop();
        }
        let Some(descriptor) = self.registry.descriptor(id).cloned() else {
            return DispatchReport::noop();
        };
        if self.write_policy(id) != Some(WritePolicy::Confirmation) {
            self.awaiting_confirmation.retain(|pending| pending != id);
            return DispatchReport::noop();
        }
        let Some(model) = self.models.get(id) else {
            return DispatchReport::noop();
        };
        if model.phase != SettingPhase::Pending {
            return DispatchReport::noop();
        }
        let Some(edited) = model.edited.clone() else {
            return DispatchReport::noop();
        };
        if let Err(error) = value::validate(&descriptor, &edited) {
            let model = self.models.get_mut(id).expect("checked above");
            model.phase = SettingPhase::Failed;
            model.error = Some(error.clone());
            self.awaiting_confirmation.retain(|pending| pending != id);
            return DispatchReport {
                outcome: Outcome::Rejected(error),
                settings_changed: true,
                preferences_changed: false,
            };
        }
        if let Some(intent) = self.intent_for_edited(id, Some(edited)) {
            self.pending_intents.insert(id.to_string(), intent);
        }
        {
            let model = self.models.get_mut(id).expect("checked above");
            model.phase = SettingPhase::Applying;
        }
        self.awaiting_confirmation.retain(|pending| pending != id);
        self.applying = true;
        crate::log::info(
            "core::store",
            "confirmed apply staged",
            &[("id", id.to_string())],
        );
        DispatchReport {
            outcome: Outcome::Accepted,
            settings_changed: true,
            preferences_changed: false,
        }
    }

    /// Licoman's own appearance preference: applied in memory immediately and
    /// persisted to Licoman's config file. This is deliberately *not* a
    /// provider-backed setting edit — Licoman's own configuration is the one
    /// thing it may write.
    fn set_appearance(&mut self, mode: AppearanceMode) -> DispatchReport {
        if self.preferences.theme == mode {
            return DispatchReport::noop();
        }
        self.preferences.theme = mode;
        let saved = crate::prefs::save_to(&self.preferences_path, &self.preferences);
        let report = DispatchReport {
            outcome: match &saved {
                Ok(()) => Outcome::Accepted,
                Err(error) => Outcome::Rejected(LicomanError::new(
                    ErrorKind::Io,
                    "The theme choice applies for this session but could not be saved.",
                    error.to_string(),
                )),
            },
            settings_changed: false,
            preferences_changed: true,
        };
        crate::log::info(
            "core::store",
            "appearance preference changed",
            &[("saved", saved.is_ok().to_string())],
        );
        report
    }

    // ---- demo / test hooks (never used by normal interaction) --------------

    /// Stand-in for a change notification: the source now holds `value` while
    /// Licoman still shows its stored value. Used by `LICOMAN_DEMO` and tests.
    pub fn simulate_external_change(&mut self, id: &str, value: SettingValue) {
        let Some(model) = self.models.get_mut(id) else {
            return;
        };
        model.external = Some(value);
        model.phase = SettingPhase::ExternalConflict;
    }

    /// Mark a row Failed with the mock write error (failure-state showcase).
    pub fn simulate_failed(&mut self, id: &str) {
        let Some(model) = self.models.get_mut(id) else {
            return;
        };
        model.phase = SettingPhase::Failed;
        model.error = Some(LicomanError::new(
            ErrorKind::WriteFailed,
            "The mock provider rejected this write.",
            format!("LICOMAN_DEMO failure showcase for {id:?}"),
        ));
    }

    /// Put a row into the Permission-required state with the honest
    /// deny-by-default explanation (permission-state showcase — the live
    /// path reaches this phase through a refused edit of an
    /// authorization-held setting; a read-only-controlled row like the
    /// catalog's `distro.packages.updates` cannot be edited at all and
    /// renders its permission notice through the unavailable path).
    pub fn simulate_permission_required(&mut self, id: &str) {
        let Some(descriptor) = self.registry.descriptor(id).cloned() else {
            return;
        };
        let Some(model) = self.models.get_mut(id) else {
            return;
        };
        let scope = descriptor
            .permission_scope
            .unwrap_or_else(|| "an authorization boundary".to_string());
        model.phase = SettingPhase::PermissionRequired;
        model.error = Some(LicomanError::new(
            ErrorKind::Permission,
            format!(
                "Requires authorization — {scope}. Licoman runs unprivileged and \
                 privileged writes are denied by default in this build. Nothing \
                 was modified."
            ),
            format!("LICOMAN_DEMO permission showcase for {id:?}"),
        ));
    }

    /// Clear transient "Applied" confirmations back to Idle (the applied
    /// value already lives in `stored`).
    pub fn acknowledge_applied(&mut self) -> bool {
        let mut changed = false;
        for model in self.models.values_mut() {
            if model.phase == SettingPhase::Applied {
                model.phase = SettingPhase::Idle;
                changed = true;
            }
        }
        changed
    }
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod model_tests;
