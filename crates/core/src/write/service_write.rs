//! The service-API mutation path (06_SAFE_WRITE_ENGINE.md §3) — **fake
//! clients only in this phase**. Design points the phase demands:
//!
//! - Exact **typed** calls only: a property set on a named endpoint. There is
//!   no command execution, no shell, no generic "call anything" surface.
//! - A successful set reply does **not** prove persistence: the client
//!   classifies the change as [`Persistence::Persistent`],
//!   [`Persistence::RuntimeOnly`] or [`Persistence::Unknown`] and the UI
//!   presents that honestly.
//! - Timeouts, cancellation semantics and readback verification are explicit;
//!   a non-idempotent mutation is **never retried automatically** — after a
//!   timeout the operation is reported failed with "the service may have
//!   applied it; refresh before retrying".
//! - The engine records a compensation entry (the before-value) for every
//!   successful service write; nothing is auto-reverted.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::descriptor::SettingDescriptor;
use crate::error::{ErrorKind, LicomanError};
use crate::read::dbus::{ClientError, PropertyValue};
use crate::value::SettingValue;
use crate::write::request::{Change, ChangeRequest, ServiceBus, ServiceEndpoint};
use crate::write::{ApplyOutcome, Persistence, conflict};

/// What one write-capable service client must provide. The fake implements
/// it in-memory; a future zbus-backed implementation (Phase 07) would map
/// `set` onto `org.freedesktop.DBus.Properties.Set` with capability checks.
pub trait ServiceWriteClient: Send + Sync {
    /// Read one property (the pre-check and the readback use the same
    /// client, so verification cannot disagree with the transport).
    fn get(&self, endpoint: &ServiceEndpoint, property: &str)
    -> Result<PropertyValue, ClientError>;

    /// Set one property to a typed value.
    fn set(
        &self,
        endpoint: &ServiceEndpoint,
        property: &str,
        value: PropertyValue,
    ) -> Result<(), ClientError>;

    /// Honest persistence classification for this property.
    fn persistence(&self, endpoint: &ServiceEndpoint, property: &str) -> Persistence;
}

/// Injectable faults for the fake client (06_SAFE_WRITE_ENGINE.md §7).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FakeFaults {
    /// The set call is denied by the service.
    pub deny_set: bool,
    /// The set call never answers (the backend's timeout turns this into a
    /// truthful timeout error).
    pub hang_set: bool,
    /// The service accepts the write but ends up holding a different value
    /// (external change after apply) — the readback catches it.
    pub change_after_apply: Option<PropertyValue>,
}

/// One fake property: its value plus how it classifies persistence.
#[derive(Debug, Clone, PartialEq)]
pub struct FakeProperty {
    pub value: PropertyValue,
    pub persistence: Persistence,
}

/// In-memory fake service. Keys are (endpoint, property) so tests can model
/// two different services in one process (the multi-service workflow case).
#[derive(Default)]
pub struct FakeServiceClient {
    state: Mutex<HashMap<(ServiceEndpoint, String), FakeProperty>>,
    faults: Mutex<FakeFaults>,
    /// How long a hanging set() sleeps before giving up on its own (the
    /// backend timeout must fire first — tests configure both).
    pub hang_duration: Duration,
}

impl FakeServiceClient {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed one property (value + persistence classification).
    pub fn seed(
        &self,
        endpoint: ServiceEndpoint,
        property: &str,
        value: PropertyValue,
        persistence: Persistence,
    ) {
        self.state.lock().expect("fake state lock").insert(
            (endpoint, property.to_string()),
            FakeProperty { value, persistence },
        );
    }

    /// Replace the fault set (tests configure the scenario before applying).
    pub fn set_faults(&self, faults: FakeFaults) {
        *self.faults.lock().expect("fake fault lock") = faults;
    }

    /// Current faults (tests assert on configuration).
    pub fn current_faults(&self) -> FakeFaults {
        self.faults.lock().expect("fake fault lock").clone()
    }

    fn current(
        &self,
        endpoint: &ServiceEndpoint,
        property: &str,
    ) -> Result<PropertyValue, ClientError> {
        self.state
            .lock()
            .expect("fake state lock")
            .get(&(endpoint.clone(), property.to_string()))
            .map(|prop| prop.value.clone())
            .ok_or_else(|| {
                ClientError::Other(format!("property {property:?} not exposed by the fake"))
            })
    }
}

impl ServiceWriteClient for FakeServiceClient {
    fn get(
        &self,
        endpoint: &ServiceEndpoint,
        property: &str,
    ) -> Result<PropertyValue, ClientError> {
        self.current(endpoint, property)
    }

    fn set(
        &self,
        endpoint: &ServiceEndpoint,
        property: &str,
        value: PropertyValue,
    ) -> Result<(), ClientError> {
        {
            let faults = self.faults.lock().expect("fake fault lock");
            if faults.deny_set {
                return Err(ClientError::Denied("fake: set refused".into()));
            }
            if faults.hang_set {
                drop(faults);
                std::thread::sleep(self.hang_duration);
                // Still record the write: a hung service may or may not have
                // applied it — exactly the ambiguity the timeout path must
                // surface ("the service may have applied it").
            }
        }
        {
            let mut state = self.state.lock().expect("fake state lock");
            let key = (endpoint.clone(), property.to_string());
            let persistence = state
                .get(&key)
                .map(|prop| prop.persistence)
                .unwrap_or(Persistence::Unknown);
            state.insert(key, FakeProperty { value, persistence });
            let faults = self.faults.lock().expect("fake fault lock");
            if let Some(external) = faults.change_after_apply.clone()
                && let Some(prop) = state.get_mut(&(endpoint.clone(), property.to_string()))
            {
                prop.value = external;
            }
        }
        Ok(())
    }

    fn persistence(&self, endpoint: &ServiceEndpoint, property: &str) -> Persistence {
        self.state
            .lock()
            .expect("fake state lock")
            .get(&(endpoint.clone(), property.to_string()))
            .map(|prop| prop.persistence)
            .unwrap_or(Persistence::Unknown)
    }
}

/// The service write backend: typed pre-check → bounded typed set → typed
/// readback, with honest persistence and no automatic retries.
pub struct ServiceWriteBackend {
    client: Arc<dyn ServiceWriteClient>,
    /// Bound for every client call (the read backend's METHOD_TIMEOUT
    /// analog). Injectable so tests exercise the timeout in milliseconds.
    pub timeout: Duration,
}

impl ServiceWriteBackend {
    pub fn new(client: Arc<dyn ServiceWriteClient>) -> Self {
        Self {
            client,
            timeout: crate::read::dbus::METHOD_TIMEOUT,
        }
    }

    /// Test constructor with a short timeout.
    pub fn with_timeout(client: Arc<dyn ServiceWriteClient>, timeout: Duration) -> Self {
        Self { client, timeout }
    }

    /// Run one client call on a worker thread, bounded by `timeout`. Bounds
    /// injected clients too (a hanging fake cannot stall the engine).
    fn call_bounded<T, F>(&self, what: &'static str, call: F) -> Result<T, ClientError>
    where
        T: Send + 'static,
        F: FnOnce(&dyn ServiceWriteClient) -> Result<T, ClientError> + Send + 'static,
    {
        let client = Arc::clone(&self.client);
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name(format!("licoman-write-{what}"))
            .spawn(move || {
                let _ = tx.send(call(&*client));
            });
        match spawned {
            Ok(_handle) => match rx.recv_timeout(self.timeout) {
                Ok(result) => result,
                Err(_) => Err(ClientError::Timeout),
            },
            Err(error) => Err(ClientError::Other(format!("worker spawn: {error}"))),
        }
    }

    pub(crate) fn apply(
        &self,
        request: &ChangeRequest,
        _descriptor: &SettingDescriptor,
    ) -> Result<ApplyOutcome, LicomanError> {
        let started = Instant::now();
        let (endpoint, property) = match &request.target {
            crate::write::request::WriteTarget::Service { endpoint, property } => {
                (endpoint, property)
            }
            other => {
                return Err(LicomanError::new(
                    ErrorKind::WriteFailed,
                    "This change is not bound to a service property.",
                    format!(
                        "service backend got target kind {:?} for {}",
                        other.kind_name(),
                        request.setting_id
                    ),
                ));
            }
        };
        if property.is_empty() {
            return Err(LicomanError::new(
                ErrorKind::Validation,
                "This setting does not name a service property, so it cannot be written.",
                format!("service target without property for {}", request.setting_id),
            ));
        }
        let change_value = match &request.change {
            Change::Set(value) => value.clone(),
            Change::RemoveOverride => {
                return Err(LicomanError::new(
                    ErrorKind::Validation,
                    "A live service property cannot be reset by removing an override; \
                     resetting writes the declared default instead. Nothing was changed.",
                    format!("RemoveOverride unsupported for {}", request.setting_id),
                ));
            }
        };

        // Owned copies: every bounded call runs on a worker thread.
        let endpoint = endpoint.clone();
        let property = property.clone();

        // --- pre-check: current value must match what the UI last saw ------
        let endpoint_for_precheck = endpoint.clone();
        let property_for_precheck = property.clone();
        let current = self
            .call_bounded("precheck", move |client| {
                client.get(&endpoint_for_precheck, &property_for_precheck)
            })
            .map_err(|error| service_error(&error, "checking the current value"))?;
        let current_value = property_value_to_setting(&current).ok_or_else(|| {
            LicomanError::new(
                ErrorKind::Unavailable,
                "The service returned an unexpected type; the change was not applied.",
                format!(
                    "{}: property {property:?} holds {current:?}",
                    request.setting_id
                ),
            )
        })?;
        match &request.before {
            Some(before) if before == &current_value => {}
            Some(before) => {
                return Err(conflict(
                    &request.setting_id,
                    format!("service holds {current_value:?}, expected {before:?}"),
                ));
            }
            None => {
                return Err(conflict(
                    &request.setting_id,
                    format!("service holds {current_value:?}; it was not read before staging"),
                ));
            }
        }

        // --- typed set, bounded; no automatic retry -------------------------
        let outgoing = setting_to_property_value(&change_value).ok_or_else(|| {
            LicomanError::new(
                ErrorKind::Validation,
                "This value's type cannot be sent to the service. Nothing was changed.",
                format!(
                    "unsupported value {change_value:?} for {}",
                    request.setting_id
                ),
            )
        })?;
        let property_for_set = property.clone();
        let endpoint_for_set = endpoint.clone();
        let outgoing_for_set = outgoing.clone();
        if let Err(error) = self.call_bounded("set", move |client| {
            client.set(&endpoint_for_set, &property_for_set, outgoing_for_set)
        }) {
            return Err(match &error {
                ClientError::Timeout => LicomanError::new(
                    ErrorKind::WriteFailed,
                    "The service did not answer in time. It may still have applied \
                     the change — refresh to see the current value before retrying. \
                     Licoman does not retry service changes on its own.",
                    format!(
                        "{}: set {property:?} timed out after {:?}",
                        request.setting_id, self.timeout
                    ),
                ),
                ClientError::Denied(_) => LicomanError::new(
                    ErrorKind::Permission,
                    "The service refused this change. Nothing was modified.",
                    format!("{}: set {property:?} denied: {error}", request.setting_id),
                ),
                other => LicomanError::new(
                    ErrorKind::WriteFailed,
                    "The service could not apply this change. Nothing was modified.",
                    format!("{}: set {property:?}: {other}", request.setting_id),
                ),
            });
        }

        // --- readback: the service is authoritative -------------------------
        let endpoint_for_read = endpoint.clone();
        let property_for_read = property.clone();
        let after = self
            .call_bounded("readback", move |client| {
                client.get(&endpoint_for_read, &property_for_read)
            })
            .map_err(|error| service_error(&error, "verifying after the write"))?;
        let after_value = property_value_to_setting(&after);
        let endpoint_for_persistence = endpoint.clone();
        let property_for_persistence = property.clone();
        let persistence = self
            .call_bounded("persistence", move |client| {
                Ok::<_, ClientError>(
                    client.persistence(&endpoint_for_persistence, &property_for_persistence),
                )
            })
            .unwrap_or(Persistence::Unknown);
        let elapsed = started.elapsed();
        let match_expected = after_value.as_ref() == Some(&change_value);
        crate::log::info(
            "core::write::service",
            "service change applied",
            &[
                ("id", request.setting_id.to_string()),
                ("verified", match_expected.to_string()),
                ("persistence", format!("{persistence:?}")),
                ("elapsed_us", elapsed.as_micros().to_string()),
            ],
        );
        Ok(ApplyOutcome {
            verified: match_expected,
            expect_absent: false,
            persistence,
            restart: request.restart,
            readback_mismatch: (!match_expected)
                .then_some(after_value.unwrap_or(SettingValue::Text(String::new()))),
            notes: vec![format!("service {}.{}", endpoint.service, property)],
        })
    }
}

/// Map a client error from a pre-check/readback to a user-safe failure.
fn service_error(error: &ClientError, what: &str) -> LicomanError {
    match error {
        ClientError::Timeout => LicomanError::new(
            ErrorKind::Unavailable,
            "The service did not answer in time. Nothing was modified; try Refresh \
             later.",
            format!("{what}: timed out"),
        ),
        ClientError::Denied(_) => LicomanError::new(
            ErrorKind::Permission,
            "The service refused this operation.",
            format!("{what}: {error}"),
        ),
        ClientError::NoBus(detail) => LicomanError::new(
            ErrorKind::Unavailable,
            "The message bus is not reachable, so service-backed settings cannot be \
             changed.",
            format!("{what}: {detail}"),
        ),
        ClientError::Other(detail) => LicomanError::new(
            ErrorKind::Unavailable,
            "The service could not be queried.",
            format!("{what}: {detail}"),
        ),
    }
}

/// Convert between the wire property shapes and Licoman's typed values.
pub(crate) fn property_value_to_setting(value: &PropertyValue) -> Option<SettingValue> {
    match value {
        PropertyValue::Str(text) => Some(SettingValue::Choice(text.clone())),
        PropertyValue::StrList(items) => Some(SettingValue::List(items.clone())),
    }
}

pub(crate) fn setting_to_property_value(value: &SettingValue) -> Option<PropertyValue> {
    match value {
        SettingValue::Choice(choice) | SettingValue::Text(choice) => {
            Some(PropertyValue::Str(choice.clone()))
        }
        SettingValue::List(items) => Some(PropertyValue::StrList(items.clone())),
        _ => None,
    }
}

/// The fake also serves the *read* side, so a demo or test reads and writes
/// one service through a single client (state and faults stay consistent).
impl crate::read::dbus::PowerProfilesClient for FakeServiceClient {
    fn locate(&self) -> Result<Option<(&'static str, &'static str, &'static str)>, ClientError> {
        let endpoint = ServiceEndpoint {
            bus: ServiceBus::System,
            service: crate::read::dbus::ENDPOINTS[0].0.to_string(),
            path: crate::read::dbus::ENDPOINTS[0].1.to_string(),
            interface: crate::read::dbus::ENDPOINTS[0].2.to_string(),
        };
        let present = self
            .state
            .lock()
            .expect("fake state lock")
            .keys()
            .any(|(known, _)| *known == endpoint);
        Ok(present.then_some(crate::read::dbus::ENDPOINTS[0]))
    }

    fn property(
        &self,
        endpoint: (&'static str, &'static str, &'static str),
        name: &str,
    ) -> Result<PropertyValue, ClientError> {
        let owned = ServiceEndpoint {
            bus: ServiceBus::System,
            service: endpoint.0.to_string(),
            path: endpoint.1.to_string(),
            interface: endpoint.2.to_string(),
        };
        self.current(&owned, name)
    }
}

/// The real zbus write client (Phase 07, Integration C; decision D52): the
/// same endpoint probing as reads, typed `Properties.Get`/`Properties.Set`
/// calls, and an honest persistence classification — the PowerProfiles
/// interface documents **no** persistence guarantee for `ActiveProfile`, so
/// this build reports `Unknown` rather than promising either way.
impl ServiceWriteClient for crate::read::dbus::ZBusPowerProfilesClient {
    fn get(
        &self,
        endpoint: &ServiceEndpoint,
        property: &str,
    ) -> Result<PropertyValue, ClientError> {
        self.property(
            (&endpoint.service, &endpoint.path, &endpoint.interface),
            property,
        )
    }

    fn set(
        &self,
        endpoint: &ServiceEndpoint,
        property: &str,
        value: PropertyValue,
    ) -> Result<(), ClientError> {
        self.set_property(
            (&endpoint.service, &endpoint.path, &endpoint.interface),
            property,
            &value,
        )
    }

    fn persistence(&self, _endpoint: &ServiceEndpoint, _property: &str) -> Persistence {
        // The interface documentation promises nothing about persistence
        // across restarts (07 Integration C: "No promise of persistence
        // across reboot unless the service guarantees it").
        Persistence::Unknown
    }
}
