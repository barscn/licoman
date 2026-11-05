//! Service-API read backend over D-Bus (05_READ_ONLY_BACKENDS.md §2.C),
//! target: `org.freedesktop.UPower.PowerProfiles` — the power-profiles-daemon
//! (official interface semantics: `ActiveProfile` string property,
//! `Profiles` string-array property, property-change signals).
//!
//! - The client is **zbus** (maintained Rust D-Bus library) behind a small
//!   trait so tests inject mocks for every required case: absent service,
//!   unresponsive service, permission/error replies, dynamically varying
//!   profile lists, and property-change signals.
//! - The real client uses the **blocking** zbus API on background threads
//!   with an explicit method timeout — an unresponsive service becomes a
//!   timeout error, never a frozen UI thread.
//! - Both the current bus name (`org.freedesktop.UPower.PowerProfiles`,
//!   object path `/org/freedesktop/UPower/PowerProfiles`) and the legacy
//!   name (`net.hadess.PowerProfiles`, `/net/hadess/PowerProfiles`) are
//!   probed; whichever answers is recorded in the source details.
//! - Name probing is passive (ListNames — never activates a service);
//!   no service is installed or started to make a demo look populated.
//! - Values are runtime state: origin [`ValueOrigin::Runtime`].

use std::sync::Arc;
use std::time::Duration;

use zbus::zvariant::OwnedValue;

use crate::descriptor::SettingDescriptor;
use crate::error::{ErrorKind, LicomanError};
use crate::modules::def::BackendBinding;
use crate::read::{
    Freshness, ReadResult, RestartEffect, SettingsReadBackend, SourceDetails, SupportState,
    ValueOrigin, Writable, fnv1a_fingerprint,
};
use crate::value::SettingValue;

/// Current and legacy well-known names, with their object paths and
/// interface names (power-profiles-daemon exposes both; the interface is
/// `org.freedesktop.UPower.PowerProfiles` on either endpoint).
pub(crate) type Endpoint = (&'static str, &'static str, &'static str);

pub(crate) const ENDPOINTS: [Endpoint; 2] = [
    (
        "org.freedesktop.UPower.PowerProfiles",
        "/org/freedesktop/UPower/PowerProfiles",
        "org.freedesktop.UPower.PowerProfiles",
    ),
    (
        "net.hadess.PowerProfiles",
        "/net/hadess/PowerProfiles",
        "org.freedesktop.UPower.PowerProfiles",
    ),
];

/// Method timeout for property reads. Generous for a live daemon, short
/// enough that an unresponsive service cannot stall a refresh pass.
pub const METHOD_TIMEOUT: Duration = Duration::from_secs(3);

/// Run one blocking D-Bus call on a worker thread and wait at most
/// [`METHOD_TIMEOUT`]. zbus 5 has no per-call timeout, so the bound lives
/// here: an unresponsive service yields [`ClientError::Timeout`] while the
/// worker (if any) finishes or dies with its socket — reads never hang the
/// refresh pass.
pub(crate) fn call_bounded<T, F>(what: &'static str, call: F) -> Result<T, ClientError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, zbus::Error> + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name(format!("licoman-dbus-{what}"))
        .spawn(move || {
            let _ = tx.send(call().map_err(map_call_error));
        })
        .map_err(|error| ClientError::Other(format!("worker spawn: {error}")))?;
    match rx.recv_timeout(METHOD_TIMEOUT) {
        Ok(result) => result,
        Err(_) => Err(ClientError::Timeout),
    }
}

/// What the backend needs from a real D-Bus connection. Mocked in tests.
pub trait PowerProfilesClient: Send + Sync {
    /// The (well-known-name, object-path, interface) triple the daemon
    /// answers on right now, or `None` when no endpoint is present on the
    /// bus. Passive: must not activate anything.
    fn locate(&self) -> Result<Option<(&'static str, &'static str, &'static str)>, ClientError>;

    /// Read a property (`ActiveProfile`, `Profiles`) from the endpoint.
    fn property(
        &self,
        endpoint: (&'static str, &'static str, &'static str),
        name: &str,
    ) -> Result<PropertyValue, ClientError>;
}

/// A D-Bus property value in the shapes PowerProfiles uses.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    Str(String),
    StrList(Vec<String>),
}

/// Client failures, kept distinct so the UI can tell the states apart
/// (05_READ_ONLY_BACKENDS.md §2.C: "Display service absence and inability
/// to connect distinctly").
#[derive(Debug, Clone, PartialEq)]
pub enum ClientError {
    /// Could not reach the bus at all.
    NoBus(String),
    /// The call timed out (unresponsive service).
    Timeout,
    /// The daemon (or the bus) refused.
    Denied(String),
    /// Anything else, with a technical detail.
    Other(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::NoBus(detail) => write!(f, "bus unreachable: {detail}"),
            ClientError::Timeout => write!(f, "timed out"),
            ClientError::Denied(detail) => write!(f, "denied: {detail}"),
            ClientError::Other(detail) => write!(f, "{detail}"),
        }
    }
}

impl ClientError {
    pub(crate) fn to_licoman(&self, what: &str) -> LicomanError {
        match self {
            ClientError::NoBus(detail) => LicomanError::new(
                ErrorKind::Unavailable,
                "The system message bus is not reachable, so service-backed settings \
                 cannot be read.",
                format!("{what}: {detail}"),
            ),
            ClientError::Timeout => LicomanError::new(
                ErrorKind::Unavailable,
                "The service did not answer in time. It may be busy or stopped; try \
                 Refresh later.",
                format!("{what}: timed out after {:?}", METHOD_TIMEOUT),
            ),
            ClientError::Denied(detail) => LicomanError::new(
                ErrorKind::Permission,
                "The service refused this read.",
                format!("{what}: {detail}"),
            ),
            ClientError::Other(detail) => LicomanError::new(
                ErrorKind::Unavailable,
                "The service could not be queried.",
                format!("{what}: {detail}"),
            ),
        }
    }
}

/// zbus-backed client (production). Connection setup and property reads use
/// the blocking API with a bounded timeout; every call site runs off the UI
/// thread.
pub struct ZBusPowerProfilesClient {
    /// `None` = system bus (default). Injectable for tests that run a
    /// private bus.
    pub connect_system: bool,
}

impl Default for ZBusPowerProfilesClient {
    fn default() -> Self {
        Self {
            connect_system: true,
        }
    }
}

impl PowerProfilesClient for ZBusPowerProfilesClient {
    fn locate(&self) -> Result<Option<(&'static str, &'static str, &'static str)>, ClientError> {
        use zbus::blocking::Proxy;
        // ListNames is passive; a service that is merely activatable does
        // not appear unless it is running (activation is never requested).
        call_bounded("locate", move || {
            let connection = zbus::blocking::Connection::system()?;
            let proxy = Proxy::new(
                &connection,
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
            )?;
            proxy.call::<_, (), Vec<String>>("ListNames", &())
        })
        .map(|names| {
            for (name, path, interface) in ENDPOINTS {
                if names.iter().any(|n| n == name) {
                    return Some((name, path, interface));
                }
            }
            None
        })
    }

    fn property(
        &self,
        endpoint: (&'static str, &'static str, &'static str),
        name: &str,
    ) -> Result<PropertyValue, ClientError> {
        self.property(endpoint, name)
    }
}

impl ZBusPowerProfilesClient {
    /// Read one property through `org.freedesktop.DBus.Properties.Get` on
    /// the endpoint's destination and object path. The names are cloned:
    /// the bounded worker thread outlives the borrow.
    pub(crate) fn property(
        &self,
        endpoint: (&str, &str, &str),
        name: &str,
    ) -> Result<PropertyValue, ClientError> {
        use zbus::blocking::Proxy;
        let property_name = name.to_string();
        let destination = endpoint.0.to_string();
        let path = endpoint.1.to_string();
        let owned: OwnedValue = call_bounded("get-property", move || {
            let connection = zbus::blocking::Connection::system()?;
            let proxy = Proxy::new(
                &connection,
                destination,
                path,
                "org.freedesktop.DBus.Properties",
            )?;
            proxy.call(
                "Get",
                &("org.freedesktop.UPower.PowerProfiles", property_name),
            )
        })?;
        property_owned_to_value(owned)
    }

    /// Write one property through `org.freedesktop.DBus.Properties.Set` on
    /// the probed endpoint (Phase 07, Integration C; decision D52). Typed
    /// property set only — the endpoint comes from the same passive probing
    /// as reads, and the value shapes are the two PowerProfiles uses.
    pub(crate) fn set_property(
        &self,
        endpoint: (&str, &str, &str),
        name: &str,
        value: &PropertyValue,
    ) -> Result<(), ClientError> {
        use zbus::blocking::Proxy;
        use zbus::zvariant::Value;
        let property_name = name.to_string();
        let interface = endpoint.2.to_string();
        let destination = endpoint.0.to_string();
        let path = endpoint.1.to_string();
        let zvalue: Value<'_> = match value {
            PropertyValue::Str(text) => Value::Str(text.clone().into()),
            PropertyValue::StrList(items) => Value::Array(
                items
                    .iter()
                    .map(|item| Value::Str(item.clone().into()))
                    .collect::<Vec<_>>()
                    .into(),
            ),
        };
        call_bounded("set-property", move || {
            let connection = zbus::blocking::Connection::system()?;
            let proxy = Proxy::new(
                &connection,
                destination,
                path,
                "org.freedesktop.DBus.Properties",
            )?;
            proxy.call::<_, _, ()>("Set", &(interface, property_name, zvalue))
        })
    }
}

fn map_call_error(error: zbus::Error) -> ClientError {
    let text = error.to_string();
    if text.contains("timed out") || text.contains("Timeout") {
        return ClientError::Timeout;
    }
    if text.contains("AccessDenied")
        || text.contains("Access Denied")
        || text.contains("not allowed")
    {
        return ClientError::Denied(text);
    }
    if text.contains("ServiceUnknown") || text.contains("NameHasNoOwner") {
        return ClientError::Other("service disappeared".into());
    }
    ClientError::Other(text)
}

/// Decode an owned D-Bus property value into the shapes PowerProfiles uses.
/// `Profiles` may be a plain string array (legacy `net.hadess` daemon) or an
/// array of `{sv}` dicts with a `Profile` entry (current daemon, signature
/// `aa{sv}` per the interface docs) — both decode to the profile name list;
/// anything else is an error, never a guess.
fn property_owned_to_value(owned: OwnedValue) -> Result<PropertyValue, ClientError> {
    use std::collections::HashMap;
    use zbus::zvariant::Value;
    let value = Value::from(owned);
    match value {
        Value::Str(s) => Ok(PropertyValue::Str(s.to_string())),
        Value::Array(items) => {
            let mut list = Vec::new();
            for item in items.iter() {
                match item.clone() {
                    Value::Str(s) => list.push(s.to_string()),
                    // Current daemon: aa{sv} — each dict carries "Profile".
                    Value::Dict(dict) => {
                        let map: HashMap<String, Value> =
                            HashMap::try_from(dict).map_err(|error| {
                                ClientError::Other(format!("Profiles dict: {error}"))
                            })?;
                        match map.get("Profile") {
                            Some(Value::Str(s)) => list.push(s.to_string()),
                            Some(other) => {
                                return Err(ClientError::Other(format!(
                                    "Profiles dict Profile type: {other:?}"
                                )));
                            }
                            None => {
                                return Err(ClientError::Other(
                                    "Profiles dict has no Profile".into(),
                                ));
                            }
                        }
                    }
                    other => {
                        return Err(ClientError::Other(format!(
                            "Profiles element type: {other:?}"
                        )));
                    }
                }
            }
            Ok(PropertyValue::StrList(list))
        }
        other => Err(ClientError::Other(format!(
            "unexpected property type: {other:?}"
        ))),
    }
}

/// A client that deterministically reports the service as absent. Used by
/// the `reads` demo so the truthful "service not running" state renders
/// without depending on the host's real services.
pub struct AbsentServiceClient;

impl PowerProfilesClient for AbsentServiceClient {
    fn locate(&self) -> Result<Option<(&'static str, &'static str, &'static str)>, ClientError> {
        Ok(None)
    }
    fn property(
        &self,
        _endpoint: (&'static str, &'static str, &'static str),
        _name: &str,
    ) -> Result<PropertyValue, ClientError> {
        Err(ClientError::Other("service absent (demo fixture)".into()))
    }
}

/// The read backend over one PowerProfiles client. The client sits behind
/// an `Arc` so every call can be bounded by a worker thread + timeout
/// (an unresponsive service — real or injected — can never stall a read).
pub struct DBusReadBackend {
    client: Arc<dyn PowerProfilesClient>,
}

impl DBusReadBackend {
    pub fn live() -> Self {
        Self {
            client: Arc::new(ZBusPowerProfilesClient::default()),
        }
    }

    pub fn with_client(client: Arc<dyn PowerProfilesClient>) -> Self {
        Self { client }
    }

    /// Run one client call on a worker thread, bounded by
    /// [`METHOD_TIMEOUT`]. This bounds *injected* clients too (the test
    /// matrix's unresponsive-service case), not just the zbus transport.
    fn call_bounded<T, F>(&self, what: &'static str, call: F) -> Result<T, ClientError>
    where
        T: Send + 'static,
        F: FnOnce(&dyn PowerProfilesClient) -> Result<T, ClientError> + Send + 'static,
    {
        let client = Arc::clone(&self.client);
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name(format!("licoman-dbus-{what}"))
            .spawn(move || {
                let _ = tx.send(call(&*client));
            });
        match spawned {
            Ok(_handle) => match rx.recv_timeout(METHOD_TIMEOUT) {
                Ok(result) => result,
                Err(_) => Err(ClientError::Timeout),
            },
            Err(error) => Err(ClientError::Other(format!("worker spawn: {error}"))),
        }
    }

    fn read_profile(&self, _descriptor: &SettingDescriptor) -> Result<ReadResult, LicomanError> {
        let endpoint = self
            .call_bounded("locate", |client| client.locate())
            .map_err(|e| e.to_licoman("locate"))?;
        let Some(endpoint_triple @ (name, path, interface)) = endpoint else {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "The power-profiles service is not running on this system, so power \
                 profiles cannot be read. It is not installed or not started; Licoman \
                 does not start system services.",
                "org.freedesktop.UPower.PowerProfiles and net.hadess.PowerProfiles are \
                 both absent from the system bus name list",
            ));
        };
        let value = self
            .call_bounded("active-profile", move |client| {
                client.property(endpoint_triple, "ActiveProfile")
            })
            .map_err(|e| e.to_licoman("ActiveProfile"))?;
        let PropertyValue::Str(profile) = value else {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "The service returned an unexpected type for the active profile.",
                format!("{name}: ActiveProfile is not a string"),
            ));
        };
        // Best-effort context facts (07 Integration C: inhibited/degraded
        // and held states where the service provides them). Failures here
        // never fail the profile read.
        let mut diagnostics = vec![format!("endpoint: {name} at {path}")];
        if let Ok(PropertyValue::Str(degraded)) = self.call_bounded("degraded", move |client| {
            client.property(endpoint_triple, "PerformanceDegraded")
        }) {
            if degraded.is_empty() {
                diagnostics.push("performance not degraded".to_string());
            } else {
                diagnostics.push(format!("performance degraded: {degraded}"));
            }
        }
        if let Ok(PropertyValue::StrList(holds)) = self.call_bounded("holds", move |client| {
            client.property(endpoint_triple, "ActiveProfileHolds")
        }) {
            diagnostics.push(format!("profile holds: {}", holds.len()));
        }
        Ok(ReadResult {
            value: SettingValue::Choice(profile.clone()),
            origin: ValueOrigin::Runtime,
            writable: Writable::No {
                reason: "Power-profile switching is not enabled in this environment.".to_string(),
            },
            freshness: Freshness::now(fnv1a_fingerprint(profile.as_bytes())),
            restart_effect: RestartEffect::None,
            details: SourceDetails {
                backend_kind: "dbus",
                target: format!("{name} {path} {interface}.ActiveProfile"),
                origin_note: Some(format!("live property from {name}")),
                imported: false,
                diagnostics,
            },
        })
    }

    /// `PerformanceDegraded` (read-only, `s`): empty when the performance
    /// profile runs normally; otherwise the service's reason string
    /// (`lap-detected`, `high-operating-temperature`, … — the interface docs
    /// advise front-ends to show a generic message for unknown values).
    fn read_degraded(&self, _descriptor: &SettingDescriptor) -> Result<ReadResult, LicomanError> {
        let endpoint = self
            .call_bounded("locate", |client| client.locate())
            .map_err(|e| e.to_licoman("locate"))?;
        let Some(endpoint_triple @ (name, path, interface)) = endpoint else {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "The power-profiles service is not running on this system, so the \
                 performance state cannot be read. Licoman does not start system \
                 services.",
                "no PowerProfiles endpoint on the system bus",
            ));
        };
        let value = self
            .call_bounded("degraded", move |client| {
                client.property(endpoint_triple, "PerformanceDegraded")
            })
            .map_err(|e| e.to_licoman("PerformanceDegraded"))?;
        let PropertyValue::Str(degraded) = value else {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "The service returned an unexpected type for the performance state.",
                format!("{name}: PerformanceDegraded is not a string"),
            ));
        };
        let display = if degraded.is_empty() {
            "not degraded".to_string()
        } else {
            degraded.clone()
        };
        Ok(ReadResult {
            value: SettingValue::Text(display),
            origin: ValueOrigin::Runtime,
            writable: Writable::No {
                reason: "The service reports this state; it cannot be set.".to_string(),
            },
            freshness: Freshness::now(fnv1a_fingerprint(degraded.as_bytes())),
            restart_effect: RestartEffect::None,
            details: SourceDetails {
                backend_kind: "dbus",
                target: format!("{name} {path} {interface}.PerformanceDegraded"),
                origin_note: Some(format!("live property from {name}")),
                imported: false,
                diagnostics: vec![format!("raw: {degraded:?}")],
            },
        })
    }

    fn read_profiles(&self, _descriptor: &SettingDescriptor) -> Result<ReadResult, LicomanError> {
        let endpoint = self
            .call_bounded("locate", |client| client.locate())
            .map_err(|e| e.to_licoman("locate"))?;
        let Some(endpoint_triple @ (name, path, interface)) = endpoint else {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "The power-profiles service is not running on this system, so the \
                 available profiles cannot be read. Licoman does not start system \
                 services.",
                "no PowerProfiles endpoint on the system bus",
            ));
        };
        let value = self
            .call_bounded("profiles", move |client| {
                client.property(endpoint_triple, "Profiles")
            })
            .map_err(|e| e.to_licoman("Profiles"))?;
        let PropertyValue::StrList(profiles) = value else {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "The service returned an unexpected type for the profile list.",
                format!("{name}: Profiles is not a string array"),
            ));
        };
        let fingerprint_material = profiles.join("\u{1f}");
        Ok(ReadResult {
            value: SettingValue::List(profiles.clone()),
            origin: ValueOrigin::Runtime,
            writable: Writable::No {
                reason: "The profile list is provided by the service.".to_string(),
            },
            freshness: Freshness::now(fnv1a_fingerprint(fingerprint_material.as_bytes())),
            restart_effect: RestartEffect::None,
            details: SourceDetails {
                backend_kind: "dbus",
                target: format!("{name} {path} {interface}.Profiles"),
                origin_note: Some(format!("live property from {name}")),
                imported: false,
                diagnostics: vec![format!("profiles: {}", profiles.len())],
            },
        })
    }
}

impl SettingsReadBackend for DBusReadBackend {
    fn kind(&self) -> &'static str {
        "dbus"
    }

    fn probe(&self, binding: &BackendBinding) -> SupportState {
        let BackendBinding::DBus { service, .. } = binding else {
            return SupportState::Unavailable {
                reason: "Not a D-Bus binding.".into(),
            };
        };
        // Only the PowerProfiles surface is implemented in this phase.
        let is_power_profiles = service == "org.freedesktop.UPower.PowerProfiles"
            || service == "net.hadess.PowerProfiles";
        if !is_power_profiles {
            return SupportState::Unavailable {
                reason: format!(
                    "Licoman reads {service:?} through its D-Bus backend only for the \
                     power-profiles service in this phase; other services are recorded \
                     but not read yet."
                ),
            };
        }
        match self.call_bounded("probe-locate", |client| client.locate()) {
            Ok(Some(_)) => SupportState::Available,
            Ok(None) => SupportState::Unavailable {
                reason: "The power-profiles service is not running (its name is not on \
                         the system bus)."
                    .into(),
            },
            Err(ClientError::NoBus(_)) => SupportState::Unavailable {
                reason: "The system message bus is not reachable.".into(),
            },
            Err(_) => SupportState::Degraded {
                note: "The bus answered but the service could not be queried; values may \
                       still be readable."
                    .into(),
            },
        }
    }

    fn read(
        &self,
        binding: &BackendBinding,
        descriptor: &SettingDescriptor,
    ) -> Result<ReadResult, LicomanError> {
        // Only the PowerProfiles surface is implemented in this phase; the
        // service name decides before the property does.
        let BackendBinding::DBus {
            service, property, ..
        } = binding
        else {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "This setting is not bound to a D-Bus service.",
                format!("dbus backend got binding kind {:?}", binding.kind_name()),
            ));
        };
        let is_power_profiles = service == "org.freedesktop.UPower.PowerProfiles"
            || service == "net.hadess.PowerProfiles";
        if !is_power_profiles {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                format!(
                    "Licoman reads {service:?} through its D-Bus backend only for the \
                     power-profiles service in this phase; other services are recorded \
                     but not read yet."
                ),
                format!("unsupported dbus service {service:?} for {}", descriptor.id),
            ));
        }
        match property.as_deref() {
            Some("ActiveProfile") => self.read_profile(descriptor),
            Some("Profiles") => self.read_profiles(descriptor),
            Some("PerformanceDegraded") => self.read_degraded(descriptor),
            Some(other) => Err(LicomanError::new(
                ErrorKind::Unavailable,
                "This service property is not read in this phase.",
                format!("unsupported dbus property {other:?} for {}", descriptor.id),
            )),
            None => Err(LicomanError::new(
                ErrorKind::Unavailable,
                "This setting does not name a service property, so it cannot be read.",
                format!("dbus binding without property for {}", descriptor.id),
            )),
        }
    }

    fn watch_paths(&self) -> Vec<std::path::PathBuf> {
        // Property changes surface through the D-Bus watch (the adapter's
        // listener thread), not the filesystem watcher.
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{Control, Scope};
    use crate::value::SettingType;
    use std::sync::Mutex;

    fn descriptor(id: &str, setting_type: SettingType, control: Control) -> SettingDescriptor {
        SettingDescriptor::new(id, Scope::Shared, "Test", "Test", "", setting_type, control)
    }

    fn binding(property: &str) -> BackendBinding {
        BackendBinding::DBus {
            service: "org.freedesktop.UPower.PowerProfiles".into(),
            path: "/org/freedesktop/UPower/PowerProfiles".into(),
            interface: "org.freedesktop.UPower.PowerProfiles".into(),
            property: Some(property.into()),
        }
    }

    /// Mock client covering the required fixture matrix (§6.3). Shared
    /// state behind an Arc so tests can vary it between reads.
    struct Mock {
        state: std::sync::Arc<Mutex<MockState>>,
    }

    #[derive(Default)]
    struct MockState {
        present: bool,
        fail_all: bool,
        denied: bool,
        profiles: Vec<String>,
        active: String,
    }

    impl Mock {
        fn absent() -> Self {
            Self {
                state: std::sync::Arc::new(Mutex::new(MockState::default())),
            }
        }
        fn live() -> (Self, std::sync::Arc<Mutex<MockState>>) {
            let state = std::sync::Arc::new(Mutex::new(MockState {
                present: true,
                profiles: vec![
                    "balanced".into(),
                    "performance".into(),
                    "power-saver".into(),
                ],
                active: "balanced".into(),
                ..MockState::default()
            }));
            (
                Self {
                    state: state.clone(),
                },
                state,
            )
        }
    }

    impl PowerProfilesClient for Mock {
        fn locate(
            &self,
        ) -> Result<Option<(&'static str, &'static str, &'static str)>, ClientError> {
            let state = self.state.lock().unwrap();
            if !state.present {
                return Ok(None);
            }
            if state.denied {
                return Err(ClientError::Denied(
                    "org.freedesktop.DBus.Error.AccessDenied".into(),
                ));
            }
            Ok(Some(ENDPOINTS[0]))
        }
        fn property(
            &self,
            _endpoint: (&'static str, &'static str, &'static str),
            name: &str,
        ) -> Result<PropertyValue, ClientError> {
            let state = self.state.lock().unwrap();
            if state.fail_all {
                return Err(ClientError::Timeout);
            }
            match name {
                "ActiveProfile" => Ok(PropertyValue::Str(state.active.clone())),
                "Profiles" => Ok(PropertyValue::StrList(state.profiles.clone())),
                _ => Err(ClientError::Other(format!("no property {name}"))),
            }
        }
    }

    #[test]
    fn absent_service_is_a_distinct_truthful_state() {
        let backend = DBusReadBackend::with_client(Arc::new(Mock::absent()));
        let error = backend
            .read(
                &binding("ActiveProfile"),
                &descriptor(
                    "d1",
                    SettingType::Choice,
                    Control::Choice { options: vec![] },
                ),
            )
            .unwrap_err();
        assert!(error.user_message.contains("not running"));
        assert!(matches!(
            backend.probe(&binding("ActiveProfile")),
            SupportState::Unavailable { .. }
        ));
    }

    #[test]
    fn unresponsive_service_times_out_without_hanging() {
        let (mock, state) = Mock::live();
        state.lock().unwrap().fail_all = true;
        let backend = DBusReadBackend::with_client(Arc::new(mock));
        let started = std::time::Instant::now();
        let error = backend
            .read(
                &binding("ActiveProfile"),
                &descriptor(
                    "d2",
                    SettingType::Choice,
                    Control::Choice { options: vec![] },
                ),
            )
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(error.user_message.contains("did not answer in time"));
    }

    #[test]
    fn denied_reply_is_a_permission_error() {
        let (mock, state) = Mock::live();
        state.lock().unwrap().denied = true;
        let backend = DBusReadBackend::with_client(Arc::new(mock));
        let error = backend
            .read(
                &binding("ActiveProfile"),
                &descriptor(
                    "d3",
                    SettingType::Choice,
                    Control::Choice { options: vec![] },
                ),
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Permission);
    }

    #[test]
    fn profile_list_varies_dynamically() {
        let (mock, state) = Mock::live();
        let backend = DBusReadBackend::with_client(Arc::new(mock));
        let first = backend
            .read(
                &binding("Profiles"),
                &descriptor("d4", SettingType::List, Control::ReadOnly),
            )
            .unwrap();
        assert_eq!(
            first.value,
            SettingValue::List(vec![
                "balanced".into(),
                "performance".into(),
                "power-saver".into()
            ])
        );
        // The daemon changes its list (e.g. AC plugged in)…
        state.lock().unwrap().profiles = vec!["balanced".into(), "power-saver".into()];
        let second = backend
            .read(
                &binding("Profiles"),
                &descriptor("d4", SettingType::List, Control::ReadOnly),
            )
            .unwrap();
        assert_eq!(
            second.value,
            SettingValue::List(vec!["balanced".into(), "power-saver".into()])
        );
        assert_ne!(first.freshness.fingerprint, second.freshness.fingerprint);
    }

    #[test]
    fn active_profile_is_runtime_origin() {
        let (mock, _state) = Mock::live();
        let backend = DBusReadBackend::with_client(Arc::new(mock));
        let result = backend
            .read(
                &binding("ActiveProfile"),
                &descriptor(
                    "d5",
                    SettingType::Choice,
                    Control::Choice { options: vec![] },
                ),
            )
            .unwrap();
        assert_eq!(result.value, SettingValue::Choice("balanced".into()));
        assert_eq!(result.origin, ValueOrigin::Runtime);
        assert!(
            result
                .details
                .target
                .contains("org.freedesktop.UPower.PowerProfiles")
        );
    }

    #[test]
    fn non_power_profiles_services_stay_recorded_but_unread() {
        let (mock, _state) = Mock::live();
        let backend = DBusReadBackend::with_client(Arc::new(mock));
        let other = BackendBinding::DBus {
            service: "org.freedesktop.portal.Desktop".into(),
            path: "/org/freedesktop/portal/desktop".into(),
            interface: "org.freedesktop.portal.Settings".into(),
            property: Some("color-scheme".into()),
        };
        let state = backend.probe(&other);
        assert!(matches!(state, SupportState::Unavailable { .. }));
        let error = backend
            .read(
                &other,
                &descriptor("d6", SettingType::Text, Control::ReadOnly),
            )
            .unwrap_err();
        assert!(error.user_message.contains("recorded but not read"));
    }
}
