//! The store: loaded setting models, dirty tracking, validation, staged and
//! immediate apply, revert/reconcile semantics, window selections and
//! Licoman's own preferences. All of it is UI-independent and synchronous;
//! the only asynchrony (a visible "Applying…" interval) is driven by the
//! adapter splitting a staged apply into begin/complete calls.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::command::{Command, DispatchReport, Outcome};
use crate::descriptor::{ApplyMode, Control, Scope, SettingDescriptor};
use crate::error::{ErrorKind, LicomanError};
use crate::prefs::{AppearanceMode, LoadedPreferences};
use crate::registry::Registry;
use crate::search::{SearchHit, SearchIndex};
use crate::state::{Readback, SettingPhase};
use crate::value::{self, SettingValue};

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
    /// Set when an apply revealed a restart requirement (descriptor metadata
    /// declares it up front; this flag records it per apply).
    pub restart_required: bool,
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
            selected_scope: Scope::Shared,
            selected_category: None,
            selected_app: None,
            selected_app_tab: 0,
            preferences: preferences.preferences,
            preference_diagnostics: preferences.diagnostics,
            preferences_path,
            applying: false,
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
    pub fn load(&mut self) {
        self.models.clear();
        self.search_index = SearchIndex::build(self.registry.descriptors());
        let descriptors: Vec<SettingDescriptor> = self.registry.descriptors().to_vec();
        for descriptor in &descriptors {
            let model = match self.registry.read(&descriptor.id) {
                Ok(value) => SettingModel {
                    stored: Some(value),
                    phase: SettingPhase::Idle,
                    ..SettingModel::default()
                },
                Err(error) => SettingModel {
                    phase: SettingPhase::Unavailable,
                    error: Some(error),
                    ..SettingModel::default()
                },
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

        if descriptor.apply_mode == ApplyMode::Immediate {
            return self.edit_immediate(id, &descriptor, value);
        }

        // Staged: a control change is only a dirty flag, never persistence.
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

    /// Immediate mode: validate, write, read back — synchronously. A failure
    /// lands the row in Failed with a user-safe message and the edit retained
    /// for Retry/Revert.
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
        let write_result = self.registry.write(id, &value);
        let readback = if write_result.is_ok() {
            Some(self.registry.read(id))
        } else {
            None
        };

        let model = self.models.get_mut(id).expect("models cover descriptors");
        model.error = None;
        model.readback = Readback::NotYet;
        match (write_result, readback) {
            (Ok(()), Some(Ok(back))) if back.equals(&value) => {
                model.stored = Some(value);
                model.edited = None;
                model.phase = SettingPhase::Applied;
                model.readback = Readback::Confirmed;
                model.restart_required = descriptor.restart_target.is_some();
            }
            (Ok(()), Some(Ok(back))) => {
                model.stored = Some(value);
                model.external = Some(back);
                model.phase = SettingPhase::ExternalConflict;
                model.readback = Readback::Mismatch;
            }
            (Ok(()), Some(Err(error))) => {
                model.stored = Some(value);
                model.edited = None;
                model.phase = SettingPhase::Failed;
                model.error = Some(error);
            }
            (Ok(()), None) => unreachable!("readback is Some whenever the write succeeded"),
            (Err(error), _) => {
                model.edited = Some(value);
                model.phase = SettingPhase::Failed;
                model.error = Some(error);
            }
        }
        crate::log::info(
            "core::store",
            "immediate apply finished",
            &[("id", id.to_string())],
        );
        DispatchReport {
            outcome: Outcome::Applied(vec![id.to_string()]),
            settings_changed: true,
            preferences_changed: false,
        }
    }

    /// Validate and stage every dirty edit, then mark the batch applying.
    /// The adapter calls [`Store::apply_complete`] after its visible interval.
    pub fn apply_begin(&mut self) -> DispatchReport {
        if self.applying {
            return DispatchReport::noop();
        }
        let ids = self.pending_ids();
        if ids.is_empty() {
            return DispatchReport::noop();
        }
        let mut rejected: Option<LicomanError> = None;
        for id in &ids {
            let Some(descriptor) = self.registry.descriptor(id).cloned() else {
                continue;
            };
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
                Ok(()) => model.phase = SettingPhase::Pending,
                Err(error) => {
                    model.phase = SettingPhase::Failed;
                    model.error = Some(error.clone());
                    rejected.get_or_insert(error);
                }
            }
        }
        let applying_now = self
            .models
            .values()
            .filter(|m| m.phase == SettingPhase::Pending)
            .count();
        if applying_now > 0 {
            for model in self.models.values_mut() {
                if model.phase == SettingPhase::Pending {
                    model.phase = SettingPhase::Applying;
                }
            }
            self.applying = true;
        }
        crate::log::info(
            "core::store",
            "apply begin",
            &[
                ("staged", ids.len().to_string()),
                ("applying", applying_now.to_string()),
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

    /// Perform the writes for rows marked Applying, then read back. Mock
    /// writes are synchronous; real backends keep this exact split later.
    pub fn apply_complete(&mut self) -> DispatchReport {
        if !self.applying {
            return DispatchReport::noop();
        }
        self.applying = false;
        let ids: Vec<String> = self
            .models
            .iter()
            .filter(|(_, m)| m.phase == SettingPhase::Applying)
            .map(|(id, _)| id.clone())
            .collect();
        let mut applied = Vec::new();
        let mut failed = Vec::new();
        for id in &ids {
            let Some(descriptor) = self.registry.descriptor(id).cloned() else {
                continue;
            };
            let edited = self.models.get(id).and_then(|m| m.edited.clone());
            let result = match edited {
                Some(edited) => {
                    let write_result = self.registry.write(id, &edited);
                    let readback = if write_result.is_ok() {
                        Some(self.registry.read(id))
                    } else {
                        None
                    };
                    Some((edited, write_result, readback))
                }
                None => None,
            };
            let Some(model) = self.models.get_mut(id) else {
                continue;
            };
            match result {
                None => model.phase = SettingPhase::Idle,
                Some((edited, write_result, readback)) => match (write_result, readback) {
                    (Ok(()), Some(Ok(back))) if back.equals(&edited) => {
                        model.stored = Some(edited);
                        model.edited = None;
                        model.phase = SettingPhase::Applied;
                        model.readback = Readback::Confirmed;
                        model.error = None;
                        model.restart_required = descriptor.restart_target.is_some();
                        applied.push(id.clone());
                    }
                    (Ok(()), Some(Ok(back))) => {
                        model.stored = Some(edited);
                        model.edited = None;
                        model.external = Some(back);
                        model.phase = SettingPhase::ExternalConflict;
                        model.readback = Readback::Mismatch;
                        failed.push(id.clone());
                    }
                    (Ok(()), Some(Err(error))) => {
                        model.stored = Some(edited);
                        model.edited = None;
                        model.phase = SettingPhase::Failed;
                        model.error = Some(error);
                        failed.push(id.clone());
                    }
                    (Ok(()), None) => {
                        unreachable!("readback is Some whenever the write succeeded")
                    }
                    (Err(error), _) => {
                        model.phase = SettingPhase::Failed;
                        model.error = Some(error);
                        failed.push(id.clone());
                    }
                },
            }
        }
        crate::log::info(
            "core::store",
            "apply complete",
            &[
                ("applied", applied.len().to_string()),
                ("failed", failed.len().to_string()),
            ],
        );
        DispatchReport {
            outcome: if applied.is_empty() && failed.is_empty() {
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
        {
            let model = self.models.get_mut(id).expect("checked above");
            model.phase = SettingPhase::Applying;
        }
        self.applying = true;
        self.apply_complete()
    }

    fn revert(&mut self, id: &str) -> DispatchReport {
        let Some(model) = self.models.get_mut(id) else {
            return DispatchReport::noop();
        };
        if model.phase == SettingPhase::Failed {
            model.edited = None;
            model.error = None;
            model.phase = SettingPhase::Idle;
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
    /// 00_START_HERE.md non-negotiable 1).
    fn reload(&mut self, id: &str) -> DispatchReport {
        if self.model(id).is_some_and(|m| m.phase.holds_pending_edit()) {
            return DispatchReport::noop();
        }
        if self.model(id).is_none() {
            return DispatchReport::noop();
        }
        match self.registry.read(id) {
            Ok(value) => {
                let model = self.models.get_mut(id).expect("checked above");
                model.stored = Some(value);
                model.external = None;
                model.error = None;
                model.phase = SettingPhase::Idle;
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
