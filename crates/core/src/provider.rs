//! The provider abstraction (02_FOUNDATION.md §2): probe, read, optional
//! write, and capabilities. A `watch` method is deliberately **absent** in
//! this phase — there is no background daemon and no idle polling; change
//! detection happens on explicit reloads and via the mock's deterministic
//! read sequencing. `Capabilities::watch` exists so later phases can declare
//! watch support without reshaping the trait.

use crate::descriptor::SettingDescriptor;
use crate::error::LicomanError;
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
/// enough for tests; they run on the UI thread in Phase 02 (mock data only).
pub trait Provider {
    /// Stable provider identifier (used in logs and diagnostics).
    fn id(&self) -> &'static str;

    /// Cheap availability check for this environment.
    fn probe(&self) -> ProbeResult;

    /// The settings this provider owns, in display order.
    fn descriptors(&self) -> Vec<SettingDescriptor>;

    /// Read the current value at the source.
    fn read(&self, id: &str) -> Result<SettingValue, LicomanError>;

    /// Write a validated value. Providers that cannot write return an error;
    /// the store consults [`Provider::probe`] capabilities first so this is
    /// only reached when a write is advertised.
    fn write(&self, id: &str, value: &SettingValue) -> Result<(), LicomanError>;
}
