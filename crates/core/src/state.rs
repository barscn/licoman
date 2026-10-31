//! The explicit lifecycle phases of a setting (02_FOUNDATION.md §4):
//!
//! ```text
//! Read -> Loaded(Idle) -> Dirty -> Validating -> Pending -> Applying -> Applied
//!                         |                          |             |
//!                         +-> Discard                +-> Failure    +-> Readback
//!                         +-> External conflict ----->  Reconcile
//! ```
//!
//! A control value changing visually is **never** a persisted setting: only
//! the transitions driven through the [`crate::model::Store`] move a phase.

/// Lifecycle phase of one setting.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum SettingPhase {
    /// Registered but not read yet.
    #[default]
    Unknown,
    /// The setting cannot work in this environment; the model carries the
    /// reason in its error.
    Unavailable,
    /// Loaded and consistent with the source.
    Idle,
    /// Edited in the UI; the edit is not validated yet.
    Dirty,
    /// Transient: validation is running for a staged edit.
    Validating,
    /// Validated staged change, waiting for the explicit Apply.
    Pending,
    /// A write is in flight (mock: between apply-begin and apply-complete).
    Applying,
    /// The provider confirmed the write (readback matched).
    Applied,
    /// The last operation failed; the model carries the error.
    Failed,
    /// The source changed outside Licoman; reconcile before editing.
    ExternalConflict,
    /// The change requires an authorization the deny-by-default privilege
    /// broker refused (06_SAFE_WRITE_ENGINE.md §4). The edit is retained but
    /// never applied; the UI offers the honest explanation.
    PermissionRequired,
}

impl SettingPhase {
    /// Phases that hold a user edit that has not been applied.
    pub fn holds_pending_edit(&self) -> bool {
        matches!(
            self,
            SettingPhase::Dirty
                | SettingPhase::Validating
                | SettingPhase::Pending
                | SettingPhase::Applying
        )
    }

    /// Whether the control should refuse input in this phase.
    pub fn blocks_editing(&self) -> bool {
        matches!(
            self,
            SettingPhase::Applying
                | SettingPhase::ExternalConflict
                | SettingPhase::PermissionRequired
                | SettingPhase::Unavailable
                | SettingPhase::Unknown
        )
    }

    /// Whether an apply/retry action is meaningful right now.
    pub fn is_busy(&self) -> bool {
        matches!(self, SettingPhase::Validating | SettingPhase::Applying)
    }
}

/// Result of the readback that follows a successful mock write.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Readback {
    /// No write happened yet (or not applicable).
    #[default]
    NotYet,
    /// Read after write returned the value we wrote.
    Confirmed,
    /// Read after write returned something else — surfaced as an external
    /// conflict instead of pretending the write stuck.
    Mismatch,
}
