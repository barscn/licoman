//! The typed read interface (05_READ_ONLY_BACKENDS.md §1): one state model
//! shared by every backend family, with provenance, freshness and the write
//! boundary carried on the result instead of being re-derived per module.
//!
//! Conceptual operations per backend: `probe` (support state for a binding),
//! `read` (typed effective value with origin), and change detection via
//! [`SettingsReadBackend::watch_paths`] — the UI adapter attaches debounced
//! filesystem watchers there; D-Bus property changes surface through the same
//! refresh path (05_READ_ONLY_BACKENDS.md §3: "a watcher, signal, or tested
//! Refresh pathway").
//!
//! Hard rules honored here: a missing setting is never a valid empty value
//! (it is an error with a reason); the core distinguishes schema defaults
//! from effective values and file overrides from imported values; and
//! nothing in this module writes anything.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::descriptor::SettingDescriptor;
use crate::error::LicomanError;
use crate::modules::def::BackendBinding;
use crate::value::SettingValue;

pub mod dbus;
pub mod file;
pub mod gsettings;
pub mod gvariant;
pub mod router;

pub use router::BackendRouter;

#[cfg(test)]
#[path = "read_matrix_tests.rs"]
mod read_matrix_tests;

/// Where an effective value came from
/// (05_READ_ONLY_BACKENDS.md §1: `default | system | user | managed |
/// runtime | unknown`). The UI must not claim a setting is user-overridden
/// unless this is [`ValueOrigin::User`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueOrigin {
    /// The schema/binary declared default; nothing overrode it.
    Default,
    /// A system-level file or database (e.g. a dconf system database).
    System,
    /// The user's own configuration (dconf user db, the user's config file).
    User,
    /// A management layer above user settings (dconf system databases in the
    /// profile, which override per dconf's profile priority).
    Managed,
    /// Live service state read over D-Bus (e.g. the active power profile).
    Runtime,
    /// Origin could not be determined (mock fallbacks, uncertain sources).
    Unknown,
}

impl ValueOrigin {
    /// Short truthful label for status lines and Source details.
    pub fn label(self) -> &'static str {
        match self {
            ValueOrigin::Default => "Default",
            ValueOrigin::System => "System",
            ValueOrigin::User => "Set by you",
            ValueOrigin::Managed => "Managed by your system",
            ValueOrigin::Runtime => "Live service state",
            ValueOrigin::Unknown => "Origin unknown",
        }
    }
}

/// Whether the setting could be written at all. Phase 05 is read-only:
/// real backends report [`Writable::No`] with the honest reason, never a
/// clickable control pretending to save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Writable {
    Yes,
    No { reason: String },
    NeedsPermission { auth: String },
    Unknown,
}

/// What a change to this setting would need to take effect. Derived from the
/// binding family (which process owns the value) and refined by the
/// descriptor's declared restart target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartEffect {
    None,
    App,
    Service,
    Session,
    System,
    Unknown,
}

/// How fresh the read is: wall-clock read time plus a deterministic source
/// fingerprint (FNV-1a over the source bytes that produced the value) so
/// stale rows can be detected without storing contents anywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Freshness {
    pub read_at: SystemTime,
    /// e.g. `fnv1a64:1a2b3c4d5e6f7081` — a fingerprint, not a secret.
    pub fingerprint: String,
}

impl Freshness {
    pub fn now(fingerprint: String) -> Self {
        Self {
            read_at: SystemTime::now(),
            fingerprint,
        }
    }
}

/// Metadata shown in the Source details disclosure. **By construction this
/// never carries file contents or setting values** — only identifiers,
/// paths, and diagnostics — so a details panel cannot leak configuration
/// secrets (05_READ_ONLY_BACKENDS.md §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDetails {
    /// Backend family: `gsettings`, `file`, `dbus`, `mock`.
    pub backend_kind: &'static str,
    /// Exact target: `schema/key`, config file path, or
    /// `service path interface.property`.
    pub target: String,
    /// How the effective value was derived (e.g. "dconf user database",
    /// "schema default", "merged from 2 imports; winner: …").
    pub origin_note: Option<String>,
    /// True when the winning value came from an imported/included file
    /// rather than the file the module names directly.
    pub imported: bool,
    /// Technical diagnostics (paths, counts, skip reasons). Never contents.
    pub diagnostics: Vec<String>,
}

/// One backend's verdict on whether it can serve a binding here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupportState {
    Available,
    /// Can serve, with a disclosed limitation (e.g. non-dconf backend env).
    Degraded {
        note: String,
    },
    /// Cannot serve at all; the reason is user-safe.
    Unavailable {
        reason: String,
    },
}

/// The result of one read: typed value plus everything the UI needs to
/// present it truthfully.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadResult {
    pub value: SettingValue,
    pub origin: ValueOrigin,
    pub writable: Writable,
    pub freshness: Freshness,
    pub restart_effect: RestartEffect,
    pub details: SourceDetails,
}

impl ReadResult {
    /// Wrap a bare value with the envelope real backends must fill in.
    pub fn new(value: SettingValue, origin: ValueOrigin, details: SourceDetails) -> Self {
        Self {
            value,
            origin,
            writable: Writable::Unknown,
            freshness: Freshness::now(fnv1a_fingerprint(&[])),
            restart_effect: RestartEffect::None,
            details,
        }
    }
}

/// One read-only backend family. Implementations are shared handles
/// (`Send + Sync`) — reads run off the UI thread; they must be bounded
/// (timeouts, size caps) and never panic on malformed input.
pub trait SettingsReadBackend: Send + Sync {
    /// Backend family name (`gsettings`, `file`, `dbus`).
    fn kind(&self) -> &'static str;

    /// Cheap support check for one binding (schema present? service on the
    /// bus? file exists?). Absence is a normal, reportable state.
    fn probe(&self, binding: &BackendBinding) -> SupportState;

    /// Read the effective value with full provenance. Bounded; failures are
    /// [`LicomanError`]s with user-safe messages, never panics.
    fn read(
        &self,
        binding: &BackendBinding,
        descriptor: &SettingDescriptor,
    ) -> Result<ReadResult, LicomanError>;

    /// Paths whose changes invalidate this backend's values. The adapter
    /// watches them (debounced) and triggers a refresh; empty for backends
    /// with no filesystem footprint.
    fn watch_paths(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// Deterministic 64-bit FNV-1a fingerprint over bytes, hex-formatted. Used
/// for freshness comparisons only — not a cryptographic hash.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Human-visible fingerprint string for [`Freshness`].
pub fn fnv1a_fingerprint(bytes: &[u8]) -> String {
    format!("fnv1a64:{:016x}", fnv1a(bytes))
}

/// Read a file with the DoS bounds every file backend must apply
/// (05_READ_ONLY_BACKENDS.md §4): size cap, no unbounded reads.
pub(crate) fn read_bounded(path: &std::path::Path, max_bytes: usize) -> std::io::Result<Vec<u8>> {
    let meta = std::fs::metadata(path)?;
    if meta.len() as usize > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            format!("file exceeds the {max_bytes} byte read limit"),
        ));
    }
    std::fs::read(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_deterministic_and_sensitive() {
        let a = fnv1a_fingerprint(b"clock-show-weekday=true");
        let b = fnv1a_fingerprint(b"clock-show-weekday=true");
        let c = fnv1a_fingerprint(b"clock-show-weekday=false");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("fnv1a64:"));
    }

    #[test]
    fn bounded_reader_rejects_oversize() {
        let dir = std::env::temp_dir().join("licoman-read-bounds-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.bin");
        std::fs::write(&path, vec![0u8; 4096]).unwrap();
        assert!(read_bounded(&path, 1024).is_err());
        assert_eq!(read_bounded(&path, 8192).unwrap().len(), 4096);
        let _ = std::fs::remove_file(&path);
    }
}
