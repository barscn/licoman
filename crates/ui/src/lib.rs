//! Licoman UI adapter — the GPUI presentation layer.
//!
//! Everything here renders the **Quiet Control** design system and talks to
//! the domain exclusively through [`licoman_core::Store`] snapshots and typed
//! [`licoman_core::Command`]s. This crate never opens configuration files,
//! spawns processes, or writes anything outside what the core already does.

pub mod actions;
pub mod app_state;
pub mod environment_watch;
pub mod gsettings_watch;
pub mod settings_watch;
pub mod theme;
pub mod ui;
pub mod windows;
