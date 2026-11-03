//! The write engine: one place applies a [`ChangeRequest`] through the right
//! backend, behind the policy gate and the privilege broker, and records a
//! bounded, redacted operation for every attempt (06_SAFE_WRITE_ENGINE.md
//! §1 lifecycle, §4 permission architecture, §6 privacy).
//!
//! "Safe apply" per backend family (06_SAFE_WRITE_ENGINE.md §8.1):
//!
//! - **file** — conflict-checked against the re-read source, structurally
//!   preserving (`toml_edit`), backed up, staged + atomic-renamed, read back
//!   and verified; restore on verification failure.
//! - **service (D-Bus, fake in this phase)** — typed pre-check, bounded typed
//!   set, typed readback; persistence reported honestly; no automatic retry
//!   of a timed-out (non-idempotent) mutation.
//! - **mock** — in-memory declared-sample values; the store verifies via its
//!   own readback.
//! - **gsettings** — refused in this build; live dconf writes are gated to
//!   Phase 07.

use std::sync::Arc;

use crate::descriptor::SettingDescriptor;
use crate::error::{ErrorKind, LicomanError};
use crate::write::broker::{DenyAllBroker, PrivilegeBroker};
use crate::write::file_write::FileWriteBackend;
use crate::write::request::WriteTarget;
use crate::write::service_write::ServiceWriteBackend;
use crate::write::{
    ApplyOutcome, ChangeRequest, OpKind, OpStatus, OperationLog, OperationRecord, WritePolicy,
};

/// The engine handle (shared by the provider; Send + Sync).
pub struct WriteEngine {
    file: FileWriteBackend,
    service: ServiceWriteBackend,
    /// GSettings backend: `Some` only where the native API is wired (live
    /// providers and gsettings-fixture tests). `None` refuses GSettings
    /// writes with an honest gate message.
    gsettings: Option<std::sync::Arc<crate::write::gsettings_write::GSettingsWriteBackend>>,
    broker: Arc<dyn PrivilegeBroker>,
    log: std::sync::Mutex<OperationLog>,
}

impl WriteEngine {
    /// Production-shaped engine (deny-by-default broker, standard timeouts,
    /// no GSettings backend — attach one explicitly where supported).
    pub fn new(file: FileWriteBackend, service: ServiceWriteBackend) -> Self {
        Self::with_broker(file, service, Arc::new(DenyAllBroker))
    }

    /// Injectable broker (tests assert the deny-by-default behavior with the
    /// real [`DenyAllBroker`]; a future authorized broker would plug in here
    /// after review).
    pub fn with_broker(
        file: FileWriteBackend,
        service: ServiceWriteBackend,
        broker: Arc<dyn PrivilegeBroker>,
    ) -> Self {
        Self {
            file,
            service,
            gsettings: None,
            broker,
            log: std::sync::Mutex::new(OperationLog::default()),
        }
    }

    /// Attach the GSettings write backend (live providers with the native
    /// layer; gsettings-fixture tests with the memory backend).
    pub fn with_gsettings(
        mut self,
        gsettings: std::sync::Arc<crate::write::gsettings_write::GSettingsWriteBackend>,
    ) -> Self {
        self.gsettings = Some(gsettings);
        self
    }

    /// The GSettings backend when attached (the provider consults its gate).
    pub fn gsettings(
        &self,
    ) -> Option<&std::sync::Arc<crate::write::gsettings_write::GSettingsWriteBackend>> {
        self.gsettings.as_ref()
    }

    /// The privilege broker in use (diagnostics).
    pub fn broker_id(&self) -> &'static str {
        self.broker.id()
    }

    /// The bounded operation log (06_SAFE_WRITE_ENGINE.md §6: ids, backend,
    /// redacted target, outcome, elapsed — never values).
    pub fn operations(&self) -> Vec<OperationRecord> {
        self.log
            .lock()
            .expect("operation log lock")
            .entries()
            .cloned()
            .collect()
    }

    /// Execute one change end to end.
    pub fn apply(
        &self,
        request: &ChangeRequest,
        descriptor: &SettingDescriptor,
    ) -> Result<ApplyOutcome, LicomanError> {
        let started = std::time::Instant::now();
        // Policy gate — defense in depth: the store already refuses
        // read-only and privileged edits; the engine refuses them again so
        // no other caller can bypass the policy (06_SAFE_WRITE_ENGINE.md §1:
        // backend + core own the policy).
        match request.policy {
            WritePolicy::ReadOnly => {
                return Err(self.refuse(request, started, OpStatus::Refused, "read-only policy"));
            }
            WritePolicy::Privileged => {
                if let Err(error) = self.broker.authorize(request) {
                    let _ = self.record(request, started, OpStatus::Refused);
                    return Err(error);
                }
            }
            WritePolicy::Immediate | WritePolicy::Staged | WritePolicy::Confirmation => {}
        }

        // Value coherence gate: the requested value must satisfy the
        // descriptor's type and bounds. This runs before any I/O so an
        // invalid value can never touch a file or a service
        // (06_SAFE_WRITE_ENGINE.md §7: "Invalid value/type/range | No IO,
        // field-level error").
        if let Some(value) = request.change.value()
            && let Err(error) = crate::value::validate(descriptor, value)
        {
            let _ = self.record(request, started, OpStatus::Refused);
            return Err(error);
        }

        let result = match &request.target {
            WriteTarget::Mock => Err(LicomanError::new(
                ErrorKind::WriteFailed,
                "Mock settings are written in memory by the provider, not by the engine.",
                format!("engine: mock target for {}", request.setting_id),
            )),
            WriteTarget::GSettings { .. } => match &self.gsettings {
                Some(backend) => backend.apply(request, descriptor),
                None => Err(LicomanError::new(
                    ErrorKind::Permission,
                    "GSettings writes are not enabled in this environment. Nothing \
                     was modified.",
                    format!(
                        "gsettings backend not attached for {} (Phase 07 gate)",
                        request.setting_id
                    ),
                )),
            },
            WriteTarget::File { .. } => self.file.apply(request, descriptor),
            WriteTarget::Service { .. } => self.service.apply(request, descriptor),
            WriteTarget::ReadOnly { kind } => Err(LicomanError::new(
                ErrorKind::WriteFailed,
                format!(
                    "Licoman reads {kind} backends but cannot change them yet. Writes are \
                     not enabled in this build. Nothing was modified."
                ),
                format!(
                    "read-only write target refused for {} ({kind})",
                    request.setting_id
                ),
            )),
        };

        let status = match &result {
            Ok(outcome) if outcome.verified => OpStatus::Succeeded,
            Ok(_) => OpStatus::LandedUnverified,
            Err(error) => match error.kind {
                crate::error::ErrorKind::Conflict => OpStatus::Conflict,
                crate::error::ErrorKind::Permission => OpStatus::Refused,
                _ => OpStatus::Failed,
            },
        };
        let _ = self.record(request, started, status);
        result
    }

    fn refuse(
        &self,
        request: &ChangeRequest,
        started: std::time::Instant,
        status: OpStatus,
        why: &str,
    ) -> LicomanError {
        let _ = self.record(request, started, status);
        LicomanError::new(
            ErrorKind::WriteFailed,
            "This setting cannot be changed in this build. Nothing was modified.",
            format!("{}: {why}", request.setting_id),
        )
    }

    /// Record one redacted operation. Deliberately narrow: ids, backend
    /// kind, redacted target, outcome and elapsed time — never values, file
    /// contents, credentials or error details
    /// (06_SAFE_WRITE_ENGINE.md §6).
    fn record(
        &self,
        request: &ChangeRequest,
        started: std::time::Instant,
        status: OpStatus,
    ) -> u64 {
        let record = OperationRecord {
            id: 0,
            kind: OpKind::Apply,
            setting_id: request.setting_id.clone(),
            module_id: request.module_id.clone(),
            backend: request.target.kind_name(),
            target: self.redact_target(&request.target),
            status,
            elapsed: started.elapsed(),
        };
        self.log.lock().expect("operation log lock").record(record)
    }

    /// Redact a target for the operation log: file paths lose their home and
    /// config-home prefixes; service targets carry no user data.
    fn redact_target(&self, target: &WriteTarget) -> String {
        match target {
            WriteTarget::Mock => "mock value".to_string(),
            WriteTarget::GSettings { schema, key } => format!("{schema}/{key}"),
            WriteTarget::Service { endpoint, property } => {
                format!("{}.{}", endpoint.service, property)
            }
            WriteTarget::File { path, key, .. } => {
                let redacted = self.file.read_backend().redact_path(path);
                format!("{redacted} :: {key}")
            }
            WriteTarget::ReadOnly { kind } => format!("<read-only:{kind}>"),
        }
    }
}
