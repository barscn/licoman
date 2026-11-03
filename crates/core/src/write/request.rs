//! The typed change contract (06_SAFE_WRITE_ENGINE.md §1). A
//! [`ChangeIntent`] is what the user asked for (built by the store from UI
//! commands); a [`ChangeRequest`] is the fully-resolved, engine-executable
//! form. Both are structured data: no raw file text, no command strings, no
//! shell fragments can enter the pipeline because there is no field that
//! could carry them.

use crate::modules::def::{FileFormat, FileResolution, RiskLevel};
use crate::read::RestartEffect;
use crate::value::SettingValue;

/// Which bus a service target lives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceBus {
    Session,
    System,
}

/// One typed D-Bus service target (data only — never a command).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceEndpoint {
    pub bus: ServiceBus,
    pub service: String,
    pub path: String,
    pub interface: String,
}

/// Where a change lands. A closed, data-only allowlist: every variant names
/// an existing backend family's identifier scheme (mirroring
/// [`crate::modules::def::BackendBinding`]), so a request can never express
/// "run this" — only "set this typed value at this named target".
#[derive(Debug, Clone, PartialEq)]
pub enum WriteTarget {
    /// The in-process mock value source (declared sample data).
    Mock,
    /// A key inside a structured configuration file.
    File {
        path: String,
        format: FileFormat,
        key: String,
        resolution: Option<FileResolution>,
    },
    /// A GSettings schema/key pair. Writes are gated to Phase 07 — the
    /// engine refuses them in this build.
    GSettings { schema: String, key: String },
    /// A property on a D-Bus service.
    Service {
        endpoint: ServiceEndpoint,
        property: String,
    },
    /// A backend family that is explicitly read-only in this build (Phase 08:
    /// Xfconf). The engine refuses every request against it — the variant
    /// exists so the request mapping stays total and the refusal is typed,
    /// not a panic or a silent drop.
    ReadOnly { kind: String },
}

impl WriteTarget {
    /// Backend family name for logs and diagnostics.
    pub fn kind_name(&self) -> &'static str {
        match self {
            WriteTarget::Mock => "mock",
            WriteTarget::File { .. } => "file",
            WriteTarget::GSettings { .. } => "gsettings",
            WriteTarget::Service { .. } => "dbus",
            WriteTarget::ReadOnly { kind } => match kind.as_str() {
                "xfconf" => "xfconf",
                _ => "read-only",
            },
        }
    }
}

/// What the change does at the target. `Set` writes a typed value;
/// `RemoveOverride` deletes a user override so the application's own default
/// applies again (the "Reset to default" semantics for file backends).
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Set(SettingValue),
    RemoveOverride,
}

impl Change {
    /// The value this change would leave at the target, if any.
    pub fn value(&self) -> Option<&SettingValue> {
        match self {
            Change::Set(value) => Some(value),
            Change::RemoveOverride => None,
        }
    }
}

/// The user-side change request built by the store: setting identity, the
/// value before (as last read), the intended change, and the expected source
/// revision. The engine turns this into a [`ChangeRequest`].
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeIntent {
    pub setting_id: String,
    /// The value the UI last saw at the source. `None` means "the key was
    /// absent" (a Set may create it; a RemoveOverride of an absent key is a
    /// no-op). The engine checks this semantically at write time.
    pub before: Option<SettingValue>,
    pub change: Change,
    /// Fingerprint of the source as last read (file backends). `None` for
    /// sources without a fingerprint (mock, service) — the semantic `before`
    /// check still applies.
    pub expected_fingerprint: Option<String>,
}

/// How the change can be undone if it succeeds. Declared per backend; the
/// engine records the compensation either way (06_SAFE_WRITE_ENGINE.md §3:
/// a best-effort compensation log, never a claimed global transaction).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackStrategy {
    /// Nothing can undo this (and the engine will not pretend otherwise).
    None,
    /// Re-applying the recorded before-value reverses it.
    InverseValue,
    /// A byte-exact backup of the affected file is kept for restore.
    BackupRestore,
}

/// The fully-resolved engine input. Built by the core (module provider +
/// engine), never by the UI; carries the impact metadata the confirmation
/// dialog shows.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeRequest {
    pub setting_id: String,
    pub module_id: String,
    pub target: WriteTarget,
    pub before: Option<SettingValue>,
    pub change: Change,
    pub expected_fingerprint: Option<String>,
    pub risk: RiskLevel,
    pub policy: crate::write::WritePolicy,
    pub restart: RestartEffect,
    pub rollback: RollbackStrategy,
}

impl ChangeRequest {
    /// Resolve an intent against a descriptor + binding. The policy is
    /// derived here (core-owned), the rollback strategy is declared per
    /// backend family.
    pub fn resolve(
        descriptor: &crate::descriptor::SettingDescriptor,
        binding: &crate::modules::def::BackendBinding,
        intent: &ChangeIntent,
        risk: RiskLevel,
    ) -> Self {
        use crate::modules::def::BackendBinding;
        let target = match binding {
            BackendBinding::Mock => WriteTarget::Mock,
            BackendBinding::GSettings { schema, key } => WriteTarget::GSettings {
                schema: schema.clone(),
                key: key.clone(),
            },
            BackendBinding::File {
                path,
                format,
                key,
                resolution,
            } => WriteTarget::File {
                path: path.clone(),
                format: *format,
                key: key.clone().unwrap_or_default(),
                resolution: *resolution,
            },
            BackendBinding::DBus {
                service,
                path,
                interface,
                property,
            } => WriteTarget::Service {
                endpoint: ServiceEndpoint {
                    bus: ServiceBus::System,
                    service: service.clone(),
                    path: path.clone(),
                    interface: interface.clone(),
                },
                property: property.clone().unwrap_or_default(),
            },
            // Explicitly read-only backend: the request exists only so the
            // engine can refuse it with a typed, honest error.
            BackendBinding::Xfconf { .. } => WriteTarget::ReadOnly {
                kind: "xfconf".to_string(),
            },
        };
        let restart = match binding {
            // A config file edit takes effect when the owning app restarts.
            BackendBinding::File { .. } => RestartEffect::App,
            _ => descriptor
                .restart_target
                .as_ref()
                .map(|target| RestartEffect::from_target(target))
                .unwrap_or(RestartEffect::None),
        };
        Self {
            setting_id: intent.setting_id.clone(),
            module_id: descriptor.module_id.clone(),
            rollback: match target {
                WriteTarget::File { .. } => RollbackStrategy::BackupRestore,
                WriteTarget::Service { .. } => RollbackStrategy::InverseValue,
                _ => RollbackStrategy::InverseValue,
            },
            target,
            before: intent.before.clone(),
            change: intent.change.clone(),
            expected_fingerprint: intent.expected_fingerprint.clone(),
            risk,
            policy: crate::write::derive_write_policy(descriptor, risk),
            restart,
        }
    }
}

impl RestartEffect {
    pub(crate) fn from_target(target: &str) -> Self {
        // The manifest's restart field names the affected target in plain
        // words ("the terminal", "the session"). Map known coarse targets;
        // anything else stays an honest Unknown with the declared text shown
        // by the UI from the descriptor.
        match target {
            "app" => RestartEffect::App,
            "service" => RestartEffect::Service,
            "session" => RestartEffect::Session,
            "system" => RestartEffect::System,
            _ => RestartEffect::Unknown,
        }
    }
}

/// What "Reset to default" concretely does for one setting
/// (06_SAFE_WRITE_ENGINE.md §5: disclose deleting an override vs writing a
/// literal value; the backend semantics decide).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResetDisclosure {
    /// The user's override (key) is removed so the application's own default
    /// applies again. Disclosed before the write.
    DeletesOverride { target: String },
    /// A literal declared default value is written.
    WritesLiteral { value: String },
    /// Reset is not available, with the honest reason.
    NotPossible { reason: String },
}

impl ResetDisclosure {
    /// The disclosure line the confirmation dialog shows.
    pub fn explanation(&self) -> String {
        match self {
            ResetDisclosure::DeletesOverride { target } => format!(
                "This removes your override ({target}) so the application's own \
                 default applies again."
            ),
            ResetDisclosure::WritesLiteral { value } => format!(
                "This writes the declared default value ({value}) as an explicit \
                 setting."
            ),
            ResetDisclosure::NotPossible { reason } => {
                format!("Reset is not available for this setting: {reason}")
            }
        }
    }
}

/// The provider's answer to "can this setting be reset, and how?".
#[derive(Debug, Clone, PartialEq)]
pub struct ResetSupport {
    pub disclosure: ResetDisclosure,
    /// The declared default (for `WritesLiteral`); `None` when undeclared.
    pub default_value: Option<SettingValue>,
    /// The change the reset performs.
    pub kind: Change,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_values_are_only_present_for_sets() {
        assert!(Change::Set(SettingValue::Bool(true)).value().is_some());
        assert!(Change::RemoveOverride.value().is_none());
    }

    #[test]
    fn disclosures_explain_themselves_in_user_words() {
        let delete = ResetDisclosure::DeletesOverride {
            target: "font.size".into(),
        };
        assert!(delete.explanation().contains("removes your override"));
        let literal = ResetDisclosure::WritesLiteral {
            value: "balanced".into(),
        };
        assert!(literal.explanation().contains("declared default"));
        let impossible = ResetDisclosure::NotPossible {
            reason: "no default is declared".into(),
        };
        assert!(impossible.explanation().contains("not available"));
    }
}
