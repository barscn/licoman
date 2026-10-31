//! The privilege boundary (06_SAFE_WRITE_ENGINE.md §4). The GUI runs as a
//! normal user: no whole-app root execution, no `sudo` prompts in the UI, no
//! password collection. Privileged changes go through a
//! [`PrivilegeBroker`]; this phase ships only the deny-by-default no-op
//! broker, so every privileged request is refused with a truthful
//! explanation.
//!
//! Declaring `[requires_root]` (or any `required_auth`) in a module is **not**
//! authorization: it routes the request to this broker, which refuses it. A
//! future broker (documented in `docs/security/threat-model.md`) would use a
//! polkit-checked minimal helper with an allowlist of operations — never a
//! general command runner — but none of that exists in this build.

use crate::error::{ErrorKind, LicomanError};
use crate::write::request::ChangeRequest;

/// Authorizes (or refuses) privileged operations. Implementations must be
/// cheap, side-effect-free until asked, and fail closed.
pub trait PrivilegeBroker: Send + Sync {
    /// Broker identity for logs and diagnostics.
    fn id(&self) -> &'static str;

    /// Decide whether this privileged change may proceed. `Err` = refused;
    /// the error carries a user-safe explanation.
    fn authorize(&self, request: &ChangeRequest) -> Result<(), LicomanError>;
}

/// The Phase 06 broker: refuses every privileged request, by design. No
/// operation is authorized, nothing is spawned, nothing is collected.
pub struct DenyAllBroker;

impl PrivilegeBroker for DenyAllBroker {
    fn id(&self) -> &'static str {
        "deny-all"
    }

    fn authorize(&self, request: &ChangeRequest) -> Result<(), LicomanError> {
        let _ = request;
        Err(LicomanError::new(
            ErrorKind::Permission,
            "This change requires elevated privileges, which Licoman does not use \
             in this build. It runs unprivileged and never asks for your password; \
             nothing was modified.",
            "denied by the deny-all privilege broker (Phase 06)",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{ApplyMode, Control, Scope, SettingDescriptor};
    use crate::modules::def::RiskLevel;
    use crate::value::SettingType;
    use crate::write::ChangeIntent;

    fn request_with_auth() -> ChangeRequest {
        let descriptor = SettingDescriptor {
            id: "distro.packages.updates".into(),
            scope: Scope::Distribution,
            category: "Packages".into(),
            label: "Updates".into(),
            description: String::new(),
            setting_type: SettingType::Bool,
            control: Control::Switch,
            apply_mode: ApplyMode::Staged,
            restart_target: None,
            permission_scope: Some("org.example.SystemAdmin".into()),
            note: None,
            min: None,
            max: None,
            keywords: Vec::new(),
            module_id: "distro.packages".into(),
        };
        ChangeRequest::resolve(
            &descriptor,
            &crate::modules::def::BackendBinding::Mock,
            &ChangeIntent {
                setting_id: descriptor.id.clone(),
                before: None,
                change: crate::write::Change::Set(crate::value::SettingValue::Bool(true)),
                expected_fingerprint: None,
            },
            RiskLevel::Low,
        )
    }

    #[test]
    fn deny_all_refuses_every_privileged_request() {
        let broker = DenyAllBroker;
        assert_eq!(broker.id(), "deny-all");
        let error = broker.authorize(&request_with_auth()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Permission);
        assert!(
            error.user_message.contains("nothing was modified")
                || error.user_message.contains("Nothing was modified")
        );
        // The resolved request's policy is Privileged because the
        // descriptor carries an authorization scope.
        let request = request_with_auth();
        assert_eq!(request.policy, crate::write::WritePolicy::Privileged);
    }
}
