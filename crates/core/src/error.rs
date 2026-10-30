//! Error semantics: every failure carries a user-safe message plus technical
//! detail for diagnostics. Raw debug data must never be the only user message
//! (02_FOUNDATION.md §2).

use std::fmt;

/// The class of a failure. The UI maps kinds to distinct truthful states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The setting cannot work in this environment at all.
    Unavailable,
    /// A value was rejected by descriptor constraints.
    Validation,
    /// The provider rejected or failed a write.
    WriteFailed,
    /// The operation would need authorization Licoman does not have.
    Permission,
    /// The source changed outside Licoman since the last read.
    Conflict,
    /// Licoman's own file I/O failed (preferences only).
    Io,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Unavailable => "unavailable",
            ErrorKind::Validation => "validation",
            ErrorKind::WriteFailed => "write-failed",
            ErrorKind::Conflict => "conflict",
            ErrorKind::Permission => "permission",
            ErrorKind::Io => "io",
        }
    }
}

/// A failure with two audiences: `user_message` is safe to render in the UI,
/// `technical` carries diagnostics for logs and detail views.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicomanError {
    pub kind: ErrorKind,
    pub user_message: String,
    pub technical: String,
}

impl LicomanError {
    pub fn new(
        kind: ErrorKind,
        user_message: impl Into<String>,
        technical: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            user_message: user_message.into(),
            technical: technical.into(),
        }
    }

    /// Convenience for unavailable settings (the most common mock case).
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::new(
            ErrorKind::Unavailable,
            reason,
            "provider reported unavailable",
        )
    }
}

impl fmt::Display for LicomanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} [{}]", self.user_message, self.kind.as_str())
    }
}

impl std::error::Error for LicomanError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_user_safe_with_kind_suffix() {
        let e = LicomanError::new(
            ErrorKind::WriteFailed,
            "The setting could not be saved.",
            "gsettings: spawn failed",
        );
        assert_eq!(
            e.to_string(),
            "The setting could not be saved. [write-failed]"
        );
    }

    #[test]
    fn unavailable_helper_sets_kind_and_technical() {
        let e = LicomanError::unavailable("No daemon detected.");
        assert_eq!(e.kind, ErrorKind::Unavailable);
        assert_eq!(e.user_message, "No daemon detected.");
        assert!(!e.technical.is_empty());
    }
}
