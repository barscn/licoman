//! The typed command vocabulary the UI dispatches (02_FOUNDATION.md §2: the
//! view adapter dispatches typed commands; it never mutates settings state
//! directly). Commands are handled by [`crate::model::Store::dispatch`].

use crate::descriptor::Scope;
use crate::error::LicomanError;
use crate::prefs::AppearanceMode;
use crate::value::SettingValue;

/// A user-intent action on the domain.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Change a setting's value. For `Immediate` descriptors this applies
    /// synchronously; for `Staged` ones it marks the row dirty.
    Edit {
        id: String,
        value: SettingValue,
    },
    /// Validate and apply every staged edit.
    ApplyAll,
    /// Drop every staged edit (dirty rows return to their stored values).
    DiscardAll,
    /// Drop one staged edit.
    Discard {
        id: String,
    },
    /// Re-run a failed apply for one setting.
    Retry {
        id: String,
    },
    /// Give up on a failed edit and return to the stored value.
    Revert {
        id: String,
    },
    /// Re-read a setting from its source (also reconciles an external
    /// conflict by accepting the source value).
    Reload {
        id: String,
    },
    /// Main-window navigation.
    SelectScope(Scope),
    SelectCategory(Option<String>),
    /// Applications-window navigation (independent of the main window).
    SelectApp {
        index: Option<usize>,
    },
    SelectAppTab(usize),
    /// Licoman's own appearance preference — a safe immediate write to
    /// Licoman's config, deliberately distinct from provider-backed setting
    /// edits (02_FOUNDATION.md §4).
    SetAppearance(AppearanceMode),
}

/// What a dispatched command did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Immediate-mode settings that were applied during this dispatch.
    Applied(Vec<String>),
    /// Accepted; staged applies continue asynchronously (apply-complete is a
    /// separate store call driven by the UI).
    Accepted,
    /// Refused; the error carries a user-safe message.
    Rejected(LicomanError),
    /// Nothing to do (already in the requested state, or not editable now).
    NoOp,
}

/// Result of a dispatch: the outcome plus which observable areas changed, so
/// the adapter emits the right events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchReport {
    pub outcome: Outcome,
    pub settings_changed: bool,
    pub preferences_changed: bool,
}

impl DispatchReport {
    pub fn noop() -> Self {
        Self {
            outcome: Outcome::NoOp,
            settings_changed: false,
            preferences_changed: false,
        }
    }
}
