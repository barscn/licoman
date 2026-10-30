//! Headless model of Licoman's window behavior (02_FOUNDATION.md tests §3):
//! one instance per window role, open-or-focus semantics, and an independent
//! navigation history per window. The GPUI layer keeps the actual window
//! handles and delegates the decisions to this model so the behavior is unit
//! testable without a display server.

use std::collections::HashMap;

/// The native windows Licoman can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowRole {
    Main,
    Applications,
    Preferences,
}

/// Outcome of an open request for a role that is tracked by the session model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenDecision {
    /// No window of this role exists yet — create one.
    OpenNew,
    /// A window of this role already exists — focus it instead. Single
    /// instance per role.
    FocusExisting,
}

#[derive(Debug, Default)]
pub struct WindowSessions {
    open: HashMap<WindowRole, ()>,
    histories: HashMap<WindowRole, Vec<String>>,
}

impl WindowSessions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Request to show a window of `role`. Returns the decision; marks the
    /// role open either way.
    pub fn request_open(&mut self, role: WindowRole) -> OpenDecision {
        match self.open.insert(role, ()) {
            Some(()) => OpenDecision::FocusExisting,
            None => OpenDecision::OpenNew,
        }
    }

    /// Record that the user closed a window of `role`. Its navigation history
    /// is kept until the role is opened again, so reopening can restore
    /// context; call [`WindowSessions::clear_history`] to drop it.
    pub fn closed(&mut self, role: WindowRole) {
        self.open.remove(&role);
    }

    pub fn is_open(&self, role: WindowRole) -> bool {
        self.open.contains_key(&role)
    }

    /// Record a navigation step in `role`'s own history.
    pub fn push_history(&mut self, role: WindowRole, entry: impl Into<String>) {
        let history = self.histories.entry(role).or_default();
        let entry = entry.into();
        if history.last().map(|last| last == &entry) != Some(true) {
            history.push(entry);
        }
    }

    pub fn history(&self, role: WindowRole) -> &[String] {
        self.histories
            .get(&role)
            .map(|h| h.as_slice())
            .unwrap_or(&[])
    }

    pub fn clear_history(&mut self, role: WindowRole) {
        self.histories.remove(&role);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use WindowRole::{Applications, Main, Preferences};

    #[test]
    fn single_instance_per_role() {
        let mut sessions = WindowSessions::new();
        assert_eq!(sessions.request_open(Applications), OpenDecision::OpenNew);
        assert_eq!(
            sessions.request_open(Applications),
            OpenDecision::FocusExisting
        );
        assert!(sessions.is_open(Applications));
        sessions.closed(Applications);
        assert!(!sessions.is_open(Applications));
        assert_eq!(sessions.request_open(Applications), OpenDecision::OpenNew);
    }

    #[test]
    fn roles_are_independent() {
        let mut sessions = WindowSessions::new();
        assert_eq!(sessions.request_open(Main), OpenDecision::OpenNew);
        assert_eq!(sessions.request_open(Applications), OpenDecision::OpenNew);
        assert_eq!(sessions.request_open(Preferences), OpenDecision::OpenNew);
        // Focusing Applications does not close Main or Preferences.
        assert_eq!(
            sessions.request_open(Applications),
            OpenDecision::FocusExisting
        );
        assert!(sessions.is_open(Main));
        assert!(sessions.is_open(Preferences));
    }

    #[test]
    fn navigation_histories_are_independent_per_window() {
        let mut sessions = WindowSessions::new();
        sessions.push_history(Main, "scope:Shared");
        sessions.push_history(Main, "scope:Desktop");
        sessions.push_history(Applications, "app:alacritty");
        assert_eq!(
            sessions.history(Main),
            &["scope:Shared".to_string(), "scope:Desktop".to_string()]
        );
        assert_eq!(
            sessions.history(Applications),
            &["app:alacritty".to_string()]
        );
        assert!(sessions.history(Preferences).is_empty());
    }

    #[test]
    fn consecutive_duplicate_entries_are_collapsed() {
        let mut sessions = WindowSessions::new();
        sessions.push_history(Main, "scope:Shared");
        sessions.push_history(Main, "scope:Shared");
        assert_eq!(sessions.history(Main).len(), 1);
    }
}
