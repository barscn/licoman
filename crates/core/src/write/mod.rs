//! The safe write engine (06_SAFE_WRITE_ENGINE.md): the typed mutation
//! contract, apply policies, outcome honesty and the bounded operation log.
//!
//! An edit has an explicit lifecycle, enforced by the engine — never by the
//! module that declared the setting:
//!
//! ```text
//! validate → resolve source + revision → conflict check → backup/compensation
//!          → apply via typed backend → read back to verify → record outcome
//! ```
//!
//! Hard rules honored here:
//!
//! - **Core owns the policy.** A module cannot mark itself safe: the policy
//!   is derived from the validated descriptor plus manifest risk/auth
//!   metadata ([`derive_write_policy`]), and `privileged` requests go
//!   through a [`broker::PrivilegeBroker`] that denies by default in this
//!   phase.
//! - **Fixture-only.** This phase writes configuration files under
//!   injected fixture roots and talks to a fake service client. No real
//!   `$HOME`, `/etc`, dconf or live D-Bus service is ever touched; GSettings
//!   writes are refused with a Phase 07 gate.
//! - **No false success.** An operation reports `Applied` only after the
//!   backend read the value back and it matches; service outcomes carry
//!   honest persistence (`runtime_only` / `persistent` / `unknown`).
//! - **Privacy.** The operation log records ids, backend kind, redacted
//!   target, outcome and elapsed time — never values, file contents or
//!   credentials.

pub mod broker;
pub mod engine;
pub mod file_write;
pub mod gsettings_write;
pub mod request;
pub mod service_write;

#[cfg(test)]
#[path = "write_matrix_tests.rs"]
mod write_matrix_tests;

#[cfg(test)]
#[path = "phase07_matrix_tests.rs"]
mod phase07_matrix_tests;

#[cfg(test)]
#[path = "phase08_matrix_tests.rs"]
mod phase08_matrix_tests;

pub use broker::{DenyAllBroker, PrivilegeBroker};
pub use engine::WriteEngine;
pub use file_write::FileWriteBackend;
pub use gsettings_write::{GSettingsDesktopGate, GSettingsWriteBackend, NativeGSettings};
pub use request::{
    Change, ChangeIntent, ChangeRequest, ResetDisclosure, ResetSupport, RollbackStrategy,
    ServiceBus, ServiceEndpoint, WriteTarget,
};
pub use service_write::ServiceWriteBackend;

use std::time::Duration;

use crate::descriptor::{ApplyMode, Control, SettingDescriptor};
use crate::error::{ErrorKind, LicomanError};
use crate::modules::def::RiskLevel;

/// How a change may reach the source (06_SAFE_WRITE_ENGINE.md §1). Derived
/// by the core from the descriptor and validated manifest metadata — a
/// manifest's own `policy` field is an input, never the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePolicy {
    /// No UI write action (unsupported or diagnostic-only).
    ReadOnly,
    /// One validated change with clear feedback.
    Immediate,
    /// Collected changes applied through the explicit Apply action.
    Staged,
    /// Show effect/target and require explicit confirmation before apply.
    Confirmation,
    /// Denied unless a narrow, authorized helper approves — the deny-by-default
    /// broker refuses everything in this phase.
    Privileged,
}

impl WritePolicy {
    /// User-safe label for status lines and dialogs.
    pub fn label(self) -> &'static str {
        match self {
            WritePolicy::ReadOnly => "Read-only",
            WritePolicy::Immediate => "Applies immediately",
            WritePolicy::Staged => "Applied together",
            WritePolicy::Confirmation => "Needs confirmation",
            WritePolicy::Privileged => "Permission required",
        }
    }
}

/// Derive the effective policy. The rules are deliberately conservative and
/// cannot be weakened by module data (06_SAFE_WRITE_ENGINE.md §1: "Backend +
/// core own the policy"):
///
/// - any authorization requirement ⇒ [`WritePolicy::Privileged`] (declaring
///   `required_auth` is **not** authorization — it is a denial until a real,
///   reviewed helper exists);
/// - read-only controls ⇒ [`WritePolicy::ReadOnly`];
/// - high declared risk ⇒ at least [`WritePolicy::Confirmation`], whatever
///   staging the module asked for;
/// - otherwise the descriptor's apply mode decides (immediate vs staged).
pub fn derive_write_policy(descriptor: &SettingDescriptor, risk: RiskLevel) -> WritePolicy {
    if descriptor.permission_scope.is_some() {
        return WritePolicy::Privileged;
    }
    if descriptor.control == Control::ReadOnly {
        return WritePolicy::ReadOnly;
    }
    if risk == RiskLevel::High {
        return WritePolicy::Confirmation;
    }
    match descriptor.apply_mode {
        ApplyMode::Immediate => WritePolicy::Immediate,
        ApplyMode::Staged => WritePolicy::Staged,
    }
}

/// Whether an applied change survives a reboot, as far as anyone can honestly
/// know (06_SAFE_WRITE_ENGINE.md §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Persistence {
    /// Written to durable storage (a config file, a persisting service).
    Persistent,
    /// Runtime state only — a reboot reverts it; the UI says so.
    RuntimeOnly,
    /// The backend cannot tell. Never presented as durable.
    #[default]
    Unknown,
}

impl Persistence {
    pub fn label(self) -> &'static str {
        match self {
            Persistence::Persistent => "persists across restarts",
            Persistence::RuntimeOnly => "runtime only — a restart reverts it",
            Persistence::Unknown => "persistence unknown",
        }
    }
}

/// What one successful apply reported. `verified` is only true when the
/// backend read the value back and it matched; the store never shows
/// "Applied" without that (or without a readback it performs itself).
#[derive(Debug, Clone, PartialEq)]
pub struct ApplyOutcome {
    /// The backend verified the result itself (file readback, service get).
    pub verified: bool,
    /// For `RemoveOverride` changes, the verification step treats "key now
    /// absent" as success.
    pub expect_absent: bool,
    pub persistence: Persistence,
    pub restart: crate::read::RestartEffect,
    /// The value the source reports after the write, when the backend
    /// disagreeed with the request (honest mismatch, never coerced).
    pub readback_mismatch: Option<crate::value::SettingValue>,
    /// Backend diagnostics (paths, targets) safe for the details panel.
    pub notes: Vec<String>,
}

impl ApplyOutcome {
    /// Outcome of the legacy in-memory mock write: unverified here — the
    /// store's own readback confirms it.
    pub fn mock_unverified() -> Self {
        Self {
            verified: false,
            expect_absent: false,
            persistence: Persistence::Persistent,
            restart: crate::read::RestartEffect::None,
            readback_mismatch: None,
            notes: Vec::new(),
        }
    }
}

/// What kind of user-visible operation a log entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Apply,
    Undo,
    Reset,
}

impl OpKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OpKind::Apply => "apply",
            OpKind::Undo => "undo",
            OpKind::Reset => "reset",
        }
    }
}

/// Terminal status of one recorded operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpStatus {
    Succeeded,
    Failed,
    Conflict,
    Refused,
    /// The write landed but the readback disagreed; the UI shows a conflict,
    /// never a success badge.
    LandedUnverified,
}

/// One bounded, redacted operation-log entry (06_SAFE_WRITE_ENGINE.md §6):
/// ids, backend kind, redacted target, outcome, elapsed time. Never values,
/// never file contents, never credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRecord {
    pub id: u64,
    pub kind: OpKind,
    pub setting_id: String,
    pub module_id: String,
    pub backend: &'static str,
    /// High-level target with home/config prefixes redacted to `~`.
    pub target: String,
    pub status: OpStatus,
    pub elapsed: Duration,
}

/// Bounded in-memory operation history (default cap 100; oldest dropped).
#[derive(Debug)]
pub struct OperationLog {
    entries: std::collections::VecDeque<OperationRecord>,
    cap: usize,
    next_id: u64,
}

impl Default for OperationLog {
    fn default() -> Self {
        Self::new(100)
    }
}

impl OperationLog {
    pub fn new(cap: usize) -> Self {
        Self {
            entries: std::collections::VecDeque::new(),
            cap: cap.max(1),
            next_id: 1,
        }
    }

    /// Record one operation and return its id.
    pub fn record(&mut self, record: OperationRecord) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let mut record = record;
        record.id = id;
        while self.entries.len() >= self.cap {
            self.entries.pop_front();
        }
        self.entries.push_back(record);
        id
    }

    pub fn entries(&self) -> impl Iterator<Item = &OperationRecord> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Map an I/O failure to the explicit failure taxonomy
/// (06_SAFE_WRITE_ENGINE.md §2.9): `ENOSPC`, `EACCES`, lost files, read-only
/// filesystems — each with a user-safe message and technical detail.
pub(crate) fn io_error_to_licoman(error: &std::io::Error, what: &str) -> LicomanError {
    let technical = format!("{what}: {error}");
    match error.kind() {
        std::io::ErrorKind::StorageFull => LicomanError::new(
            ErrorKind::WriteFailed,
            "The disk is full, so the change could not be saved. Your original \
             configuration is unchanged.",
            technical,
        ),
        std::io::ErrorKind::PermissionDenied => LicomanError::new(
            ErrorKind::Permission,
            "Permission was denied while writing the configuration. Nothing was \
             modified.",
            technical,
        ),
        std::io::ErrorKind::NotFound => LicomanError::new(
            ErrorKind::WriteFailed,
            "The configuration file disappeared before the change could be \
             written. Nothing was modified; refresh to re-read it.",
            technical,
        ),
        std::io::ErrorKind::ReadOnlyFilesystem => LicomanError::new(
            ErrorKind::Permission,
            "The configuration is on a read-only filesystem, so it cannot be \
             changed. Nothing was modified.",
            technical,
        ),
        _ => LicomanError::new(
            ErrorKind::WriteFailed,
            "The configuration could not be written. Nothing was modified.",
            technical,
        ),
    }
}

/// A conflict: the source changed outside Licoman since the last read. The
/// write is aborted — the user decides between Reload and manual resolution;
/// Licoman never overwrites external changes by default
/// (06_SAFE_WRITE_ENGINE.md §5).
pub(crate) fn conflict(setting_id: &str, detail: String) -> LicomanError {
    LicomanError::new(
        ErrorKind::Conflict,
        "This setting changed outside Licoman since it was read. Nothing was \
         modified — reload to see the current value or compare the changes.",
        format!("conflict for {setting_id}: {detail}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{ApplyMode, Control, Scope};
    use crate::value::SettingType;

    fn descriptor(
        apply_mode: ApplyMode,
        control: Control,
        auth: Option<&str>,
    ) -> SettingDescriptor {
        SettingDescriptor {
            id: "test.setting".into(),
            scope: Scope::Shared,
            category: "Test".into(),
            label: "Test".into(),
            description: String::new(),
            setting_type: SettingType::Bool,
            control,
            apply_mode,
            restart_target: None,
            permission_scope: auth.map(str::to_string),
            note: None,
            min: None,
            max: None,
            keywords: Vec::new(),
            module_id: String::new(),
        }
    }

    #[test]
    fn policy_derivation_cannot_be_weakened_by_the_module() {
        // Authorization always wins, whatever the module declares.
        let authed = descriptor(
            ApplyMode::Immediate,
            Control::Switch,
            Some("org.example.Admin"),
        );
        assert_eq!(
            derive_write_policy(&authed, RiskLevel::Low),
            WritePolicy::Privileged
        );
        // High risk forces confirmation even for staged/immediate modules.
        let risky = descriptor(ApplyMode::Immediate, Control::Switch, None);
        assert_eq!(
            derive_write_policy(&risky, RiskLevel::High),
            WritePolicy::Confirmation
        );
        // Plain staged and immediate rows keep their modes.
        let staged = descriptor(ApplyMode::Staged, Control::Switch, None);
        assert_eq!(
            derive_write_policy(&staged, RiskLevel::Low),
            WritePolicy::Staged
        );
        let immediate = descriptor(ApplyMode::Immediate, Control::Switch, None);
        assert_eq!(
            derive_write_policy(&immediate, RiskLevel::Low),
            WritePolicy::Immediate
        );
        // Read-only controls never become writable.
        let ro = descriptor(ApplyMode::Immediate, Control::ReadOnly, None);
        assert_eq!(
            derive_write_policy(&ro, RiskLevel::Low),
            WritePolicy::ReadOnly
        );
    }

    #[test]
    fn operation_log_is_bounded_and_ids_increase() {
        let mut log = OperationLog::new(3);
        for index in 0..5 {
            log.record(OperationRecord {
                id: 0,
                kind: OpKind::Apply,
                setting_id: format!("s{index}"),
                module_id: "m".into(),
                backend: "mock",
                target: "t".into(),
                status: OpStatus::Succeeded,
                elapsed: Duration::from_millis(1),
            });
        }
        assert_eq!(log.len(), 3);
        let ids: Vec<u64> = log.entries().map(|e| e.id).collect();
        assert_eq!(ids, vec![3, 4, 5]);
        assert_eq!(log.entries().next().unwrap().setting_id, "s2");
    }

    #[test]
    fn io_errors_map_to_distinct_truthful_kinds() {
        let full = io_error_to_licoman(
            &std::io::Error::from(std::io::ErrorKind::StorageFull),
            "stage",
        );
        assert_eq!(full.kind, ErrorKind::WriteFailed);
        assert!(full.user_message.contains("disk is full"));
        let denied = io_error_to_licoman(
            &std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            "stage",
        );
        assert_eq!(denied.kind, ErrorKind::Permission);
        assert!(denied.user_message.contains("Nothing was modified"));
    }
}
