//! The provider abstraction (02_FOUNDATION.md §2): probe, read, optional
//! write, and capabilities. Phase 05 adds the typed read interface
//! (05_READ_ONLY_BACKENDS.md §1): `read_detailed` returns the value with
//! origin, freshness, writability and source details, and `read_latency`
//! tells the store whether a read is safe on the UI thread (mock) or must
//! run on a background executor (file, GSettings, D-Bus).

use crate::descriptor::SettingDescriptor;
use crate::error::LicomanError;
use crate::read::ReadResult;
use crate::value::SettingValue;

/// What a provider (or one of its settings) can do. Fail-closed: everything
/// defaults to false.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub read: bool,
    pub write: bool,
    /// Reserved for later phases; no Phase 02 provider watches.
    pub watch: bool,
}

/// Where a read may run. `Background` reads do real I/O (files, D-Bus) and
/// are bounded by timeouts; the store defers them to a background pass and
/// publishes results as they arrive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadLatency {
    /// In-memory, effectively free (mock sources).
    Immediate,
    /// Real I/O: must run off the UI thread.
    Background,
}

/// Result of probing a provider's availability in this environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeStatus {
    Available,
    Unavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub status: ProbeStatus,
    pub capabilities: Capabilities,
}

impl ProbeResult {
    pub fn available(capabilities: Capabilities) -> Self {
        Self {
            status: ProbeStatus::Available,
            capabilities,
        }
    }

    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            status: ProbeStatus::Unavailable {
                reason: reason.into(),
            },
            capabilities: Capabilities::default(),
        }
    }

    pub fn is_available(&self) -> bool {
        matches!(self.status, ProbeStatus::Available)
    }
}

/// One backend over a set of settings. Implementations must be deterministic
/// enough for tests; `Immediate` reads run on the UI thread, `Background`
/// reads run on background executors.
pub trait Provider {
    /// Stable provider identifier (used in logs and diagnostics).
    fn id(&self) -> &'static str;

    /// Cheap availability check for this environment.
    fn probe(&self) -> ProbeResult;

    /// The settings this provider owns, in display order.
    fn descriptors(&self) -> Vec<SettingDescriptor>;

    /// Read the current value at the source (value only; see
    /// [`Provider::read_detailed`]).
    fn read(&self, id: &str) -> Result<SettingValue, LicomanError>;

    /// Read the current value with full provenance. The default maps a
    /// plain [`Provider::read`] onto a minimal envelope with unknown origin.
    fn read_detailed(&self, id: &str) -> Result<ReadResult, LicomanError> {
        let value = self.read(id)?;
        Ok(ReadResult::new(
            value,
            crate::read::ValueOrigin::Unknown,
            crate::read::SourceDetails {
                backend_kind: "unknown",
                target: id.to_string(),
                origin_note: None,
                imported: false,
                diagnostics: Vec::new(),
            },
        ))
    }

    /// Whether reads for this setting are Immediate (mock) or Background
    /// (real I/O). Default: Immediate.
    fn read_latency(&self, _id: &str) -> ReadLatency {
        ReadLatency::Immediate
    }

    /// Write a validated value. Providers that cannot write return an error;
    /// the store consults [`Provider::probe`] capabilities first so this is
    /// only reached when a write is advertised.
    fn write(&self, id: &str, value: &SettingValue) -> Result<(), LicomanError>;
}
