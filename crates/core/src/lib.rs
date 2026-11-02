//! Licoman core — the UI-independent settings domain.
//!
//! This crate **must not depend on GPUI** (00_START_HERE.md non-negotiable 2).
//! It holds everything that is true about Licoman regardless of presentation:
//!
//! - [`value`]: the typed setting values Licoman understands.
//! - [`descriptor`]: read-only setting descriptors (identity, scope, control,
//!   apply-mode and restart/permission metadata).
//! - [`state`]: the explicit lifecycle phases of a setting.
//! - [`model`]: the [`Store`] — loaded setting models, dirty tracking,
//!   validation, staged/immediate apply, revert/reconcile semantics, window
//!   selections and Licoman's own preferences.
//! - [`command`]: the typed command vocabulary the UI dispatches.
//! - [`provider`]/[`registry`]: the provider abstraction (probe / read /
//!   optional write / capabilities) and the registry that routes by stable
//!   setting ID.
//! - [`discovery`]: read-only environment, service and installed-application
//!   discovery (Phase 03) — typed snapshots with evidence and confidence.
//! - [`modules`]: declarative module manifests and the registry
//!   (Phase 04) — validated, data-only definitions, applicability and
//!   provenance; the provider bridge binds mock values in this phase.
//! - [`mock`]: a deterministic mock provider for visual and interaction
//!   testing without any installed software.
//! - [`prefs`]: Licoman-only preferences (the one thing Licoman may persist).
//! - [`log`]: structured, level-filtered logging (no values, no credentials).
//! - [`window_sessions`]: headless model of single-instance window and
//!   navigation-history behavior.
//!
//! Presentation crates observe read-only state from here and dispatch typed
//! [`command::Command`]s; they never open `/etc`, invoke `gsettings`, or edit
//! external TOML files themselves.

pub mod command;
pub mod descriptor;
pub mod discovery;
pub mod error;
pub mod hyprland;
pub mod log;
pub mod mock;
pub mod model;
pub mod modules;
pub mod prefs;
pub mod provider;
pub mod read;
pub mod registry;
pub mod search;
pub mod state;
pub mod value;
pub mod window_sessions;
pub mod write;

pub use command::{Command, DispatchReport, Outcome};
pub use descriptor::{ApplyMode, Control, Scope, SettingDescriptor};
pub use error::{ErrorKind, LicomanError};
pub use model::{SettingModel, Store};
pub use provider::{Capabilities, ProbeResult, ProbeStatus, Provider};
pub use registry::Registry;
pub use state::{Readback, SettingPhase};
pub use value::{SettingType, SettingValue};
pub use window_sessions::WindowRole;
pub use write::{
    ApplyOutcome, ChangeIntent, ChangeRequest, Persistence, ResetDisclosure, ResetSupport,
    WritePolicy,
};

/// Version of Licoman's in-memory settings type definitions (descriptors,
/// values, phases). Bump when a change would make persisted or exchanged
/// setting data from an older build ambiguous.
pub const SETTINGS_MODEL_VERSION: u32 = 1;
