//! The backend router: one place maps a validated [`BackendBinding`] to the
//! read backend family that serves it, and wraps every read in the common
//! envelope rules (writability — Phase 05 is read-only — restart effects,
//! logging that never carries values).

use std::sync::Arc;

use crate::descriptor::SettingDescriptor;
use crate::error::{ErrorKind, LicomanError};
use crate::modules::def::BackendBinding;
use crate::read::dbus::DBusReadBackend;
use crate::read::file::FileReadBackend;
use crate::read::gsettings::GSettingsReadBackend;
use crate::read::{ReadResult, SettingsReadBackend, SupportState};

/// Shared handle to the router (the provider and the watchers clone it).
#[derive(Clone)]
pub struct BackendRouter {
    inner: Arc<Inner>,
}

struct Inner {
    gsettings: GSettingsReadBackend,
    file: FileReadBackend,
    dbus: DBusReadBackend,
}

impl BackendRouter {
    /// Production router: all three backends built from the environment.
    pub fn live() -> Self {
        Self::new(
            GSettingsReadBackend::from_env(),
            FileReadBackend::from_env(),
            DBusReadBackend::live(),
        )
    }

    /// Injectable constructor for fixtures and demos.
    pub fn new(
        gsettings: GSettingsReadBackend,
        file: FileReadBackend,
        dbus: DBusReadBackend,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                gsettings,
                file,
                dbus,
            }),
        }
    }

    fn backend_for(
        &self,
        binding: &BackendBinding,
    ) -> Result<&dyn SettingsReadBackend, LicomanError> {
        let inner = &self.inner;
        // Each branch returns a reference with the backing struct's lifetime
        // tied to the Arc; transmute-free approach: dispatch through a
        // closure that borrows `inner`.
        match binding {
            BackendBinding::Mock => Err(LicomanError::new(
                ErrorKind::Unavailable,
                "Mock settings do not use the read backends.",
                "router: mock binding",
            )),
            BackendBinding::GSettings { .. } => Ok(&inner.gsettings),
            BackendBinding::File { .. } => Ok(&inner.file),
            BackendBinding::DBus { .. } => Ok(&inner.dbus),
        }
    }

    /// Probe one binding's support state in this environment.
    pub fn probe(&self, binding: &BackendBinding) -> SupportState {
        match binding {
            BackendBinding::Mock => SupportState::Available,
            _ => match self.backend_for(binding) {
                Ok(backend) => backend.probe(binding),
                Err(_) => SupportState::Unavailable {
                    reason: "This binding kind is not supported.".into(),
                },
            },
        }
    }

    /// Read one setting through the owning backend family.
    pub fn read(
        &self,
        binding: &BackendBinding,
        descriptor: &SettingDescriptor,
    ) -> Result<ReadResult, LicomanError> {
        if matches!(binding, BackendBinding::Mock) {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "Mock settings are served by the module provider, not the read backends.",
                "router: mock binding read",
            ));
        }
        let backend = self.backend_for(binding)?;
        let result = backend.read(binding, descriptor)?;
        crate::log::info(
            "core::read",
            "setting read",
            &[
                ("id", descriptor.id.to_string()),
                ("backend", backend.kind().to_string()),
                ("origin", format!("{:?}", result.origin)),
            ],
        );
        Ok(result)
    }

    /// Union of every backend's watch paths (files whose changes invalidate
    /// values). The adapter watches them with debouncing.
    pub fn watch_paths(&self) -> Vec<std::path::PathBuf> {
        let mut paths = Vec::new();
        paths.extend(self.inner.gsettings.watch_paths());
        paths.extend(self.inner.file.watch_paths());
        paths.extend(self.inner.dbus.watch_paths());
        paths.sort();
        paths.dedup();
        paths
    }
}
