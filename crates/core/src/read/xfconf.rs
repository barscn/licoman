//! Xfconf read backend (Phase 08, Wave A.3): the Xfce configuration system,
//! served by `xfconfd` on the **session** bus. The official mechanism is a
//! small documented D-Bus API on `org.xfce.Xfconf` (`/org/xfce/Xfconf`,
//! interface `org.xfce.Xfconf`):
//!
//! - `ListChannels() → as` — channels with stored state (a channel exists
//!   once any property has been recorded; presence is validated per read,
//!   never assumed).
//! - `GetProperty(channel, property) → v` / `PropertyExists(channel,
//!   property) → b` / `GetAllProperties(channel, base) → a{sv}`.
//! - `PropertyChanged(channel, property, value)` and `PropertyRemoved(
//!   channel, property)` signals — the change notifications the UI
//!   debounces into a refresh.
//!
//! Phase 08 scope: **explicitly read-only**. `SetProperty` exists in the
//! official API and xfconfd would persist it, but persistence provenance and
//! rollback for Xfconf are not proven in this build, so every row reports
//! the honest read-only reason. Writes are structurally refused twice (the
//! provider before `ChangeRequest::resolve`, the engine by
//! [`crate::write::WriteTarget::ReadOnly`]).
//!
//! Structure mirrors the PowerProfiles backend: the client sits behind a
//! trait so fixtures inject every state, the real client runs the blocking
//! zbus API on bounded worker threads, absence/timeout/denial are distinct
//! states, and values are runtime state ([`ValueOrigin::Runtime`]) — the
//! daemon merges user overrides over package defaults and Licoman reads the
//! effective value.

use std::sync::Arc;

use zbus::zvariant::OwnedValue;

use crate::descriptor::SettingDescriptor;
use crate::error::{ErrorKind, LicomanError};
use crate::modules::def::BackendBinding;
use crate::read::{
    Freshness, ReadResult, RestartEffect, SettingsReadBackend, SourceDetails, SupportState,
    ValueOrigin, Writable, fnv1a_fingerprint,
};
use crate::value::SettingValue;

pub use crate::read::dbus::ClientError;

/// The well-known Xfconf names (service, object path, interface).
pub const XFCONF_SERVICE: &str = "org.xfce.Xfconf";
pub const XFCONF_PATH: &str = "/org/xfce/Xfconf";
pub const XFCONF_INTERFACE: &str = "org.xfce.Xfconf";

/// Method timeout for Xfconf calls. Same bound as the PowerProfiles client:
/// generous for a live daemon, short enough that an unresponsive service
/// cannot stall a refresh pass.
pub const METHOD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Run one blocking D-Bus call on a worker thread and wait at most
/// [`METHOD_TIMEOUT`]. Same bound as the dbus module (zbus 5 has no per-call
/// timeout); an unresponsive `xfconfd` yields [`ClientError::Timeout`].
pub(crate) fn call_bounded<T, F>(what: &'static str, call: F) -> Result<T, ClientError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, zbus::Error> + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name(format!("licoman-xfconf-{what}"))
        .spawn(move || {
            let _ = tx.send(call().map_err(map_call_error));
        })
        .map_err(|error| ClientError::Other(format!("worker spawn: {error}")))?;
    match rx.recv_timeout(METHOD_TIMEOUT) {
        Ok(result) => result,
        Err(_) => Err(ClientError::Timeout),
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

/// A decoded Xfconf property value. Xfconf stores GVariant-typed values;
/// these are the shapes the daemon actually serves for desktop settings.
#[derive(Debug, Clone, PartialEq)]
pub enum XfconfValue {
    Bool(bool),
    /// Signed (`i`, `x`) and unsigned (`u`, `t`, `y`, `n`, `q`) integers,
    /// widened; the backend clamps to the descriptor's range checks later.
    Int(i64),
    Double(f64),
    Str(String),
    StrList(Vec<String>),
}

/// Decode an owned GVariant value into an [`XfconfValue`]. Anything the
/// daemon serves that Licoman cannot represent is an error, never a guess.
pub fn decode_owned_value(owned: OwnedValue) -> Result<XfconfValue, ClientError> {
    use zbus::zvariant::Value;
    let value = Value::from(owned);
    match value {
        Value::Bool(b) => Ok(XfconfValue::Bool(b)),
        Value::U8(v) => Ok(XfconfValue::Int(i64::from(v))),
        Value::I16(v) => Ok(XfconfValue::Int(i64::from(v))),
        Value::U16(v) => Ok(XfconfValue::Int(i64::from(v))),
        Value::I32(v) => Ok(XfconfValue::Int(i64::from(v))),
        Value::U32(v) => Ok(XfconfValue::Int(i64::from(v))),
        Value::I64(v) => Ok(XfconfValue::Int(v)),
        Value::U64(v) => i64::try_from(v)
            .map(XfconfValue::Int)
            .map_err(|_| ClientError::Other(format!("integer out of range: {v}"))),
        Value::F64(v) => Ok(XfconfValue::Double(v)),
        Value::Str(s) => Ok(XfconfValue::Str(s.to_string())),
        Value::Array(items) => {
            let mut list = Vec::new();
            for item in items.iter() {
                match item.clone() {
                    Value::Str(s) => list.push(s.to_string()),
                    other => {
                        return Err(ClientError::Other(format!(
                            "string-array element type: {other:?}"
                        )));
                    }
                }
            }
            Ok(XfconfValue::StrList(list))
        }
        other => Err(ClientError::Other(format!(
            "unsupported Xfconf value type: {other:?}"
        ))),
    }
}

/// What the backend needs from a real Xfconf connection. Mocked in tests;
/// every method runs on a bounded worker thread.
pub trait XfconfClient: Send + Sync {
    /// Whether the Xfconf daemon is **already running** (its well-known name
    /// is on the session bus). Passive ListNames — sending a method call to
    /// an activatable name would auto-start xfconfd, which Licoman must
    /// never do to make a setting look available.
    fn locate(&self) -> Result<bool, ClientError>;

    /// Channels that currently have stored state. Only called after a
    /// successful [`Self::locate`]; still passive for running services.
    fn list_channels(&self) -> Result<Vec<String>, ClientError>;

    /// Read one property. A missing channel/property is a [`ClientError`]
    /// (the daemon answers missing properties with an error reply).
    fn get_property(&self, channel: &str, property: &str) -> Result<XfconfValue, ClientError>;

    /// Whether the property exists in the channel's effective state.
    fn property_exists(&self, channel: &str, property: &str) -> Result<bool, ClientError>;
}

/// zbus-backed client (production): session bus, blocking API on bounded
/// worker threads, no activation (the daemon must already be running).
#[derive(Default)]
pub struct ZBusXfconfClient;

impl XfconfClient for ZBusXfconfClient {
    fn locate(&self) -> Result<bool, ClientError> {
        call_bounded("locate", || {
            use zbus::blocking::Proxy;
            let connection = zbus::blocking::Connection::session()?;
            let proxy = Proxy::new(
                &connection,
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
            )?;
            let names: Vec<String> = proxy.call::<_, (), Vec<String>>("ListNames", &())?;
            Ok(names.iter().any(|name| name == XFCONF_SERVICE))
        })
    }

    fn list_channels(&self) -> Result<Vec<String>, ClientError> {
        call_bounded("list-channels", || {
            use zbus::blocking::Proxy;
            let connection = zbus::blocking::Connection::session()?;
            let proxy = Proxy::new(&connection, XFCONF_SERVICE, XFCONF_PATH, XFCONF_INTERFACE)?;
            proxy.call::<_, (), Vec<String>>("ListChannels", &())
        })
    }

    fn get_property(&self, channel: &str, property: &str) -> Result<XfconfValue, ClientError> {
        let channel = channel.to_string();
        let property = property.to_string();
        let owned: OwnedValue = call_bounded("get-property", move || {
            use zbus::blocking::Proxy;
            let connection = zbus::blocking::Connection::session()?;
            let proxy = Proxy::new(&connection, XFCONF_SERVICE, XFCONF_PATH, XFCONF_INTERFACE)?;
            proxy.call("GetProperty", &(channel, property))
        })?;
        decode_owned_value(owned)
    }

    fn property_exists(&self, channel: &str, property: &str) -> Result<bool, ClientError> {
        let channel = channel.to_string();
        let property = property.to_string();
        call_bounded("property-exists", move || {
            use zbus::blocking::Proxy;
            let connection = zbus::blocking::Connection::session()?;
            let proxy = Proxy::new(&connection, XFCONF_SERVICE, XFCONF_PATH, XFCONF_INTERFACE)?;
            proxy.call::<_, _, bool>("PropertyExists", &(channel, property))
        })
    }
}

/// A client that reports the Xfconf service as absent — the `reads-xfce`
/// demo's truthful "service not running" state.
pub struct AbsentXfconfClient;

impl XfconfClient for AbsentXfconfClient {
    fn locate(&self) -> Result<bool, ClientError> {
        Ok(false)
    }
    fn list_channels(&self) -> Result<Vec<String>, ClientError> {
        Err(ClientError::Other("service absent (demo fixture)".into()))
    }
    fn get_property(&self, _channel: &str, _property: &str) -> Result<XfconfValue, ClientError> {
        Err(ClientError::Other("service absent (demo fixture)".into()))
    }
    fn property_exists(&self, _channel: &str, _property: &str) -> Result<bool, ClientError> {
        Err(ClientError::Other("service absent (demo fixture)".into()))
    }
}

/// The read backend over one Xfconf client (explicitly read-only in this
/// phase — see the module documentation).
pub struct XfconfReadBackend {
    client: Arc<dyn XfconfClient>,
}

impl XfconfReadBackend {
    pub fn live() -> Self {
        Self {
            client: Arc::new(ZBusXfconfClient),
        }
    }

    pub fn with_client(client: Arc<dyn XfconfClient>) -> Self {
        Self { client }
    }

    /// Run one client call on a worker thread, bounded by
    /// [`METHOD_TIMEOUT`] — injected clients included.
    fn call_bounded<T, F>(&self, what: &'static str, call: F) -> Result<T, ClientError>
    where
        T: Send + 'static,
        F: FnOnce(&dyn XfconfClient) -> Result<T, ClientError> + Send + 'static,
    {
        let client = Arc::clone(&self.client);
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name(format!("licoman-xfconf-{what}"))
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

    fn service_error(&self) -> LicomanError {
        LicomanError::new(
            ErrorKind::Unavailable,
            "The Xfconf configuration service (xfconfd) is not running on this system, so \
             Xfce settings cannot be read. It is not installed or not started; Licoman does \
             not start services.",
            format!("{XFCONF_SERVICE} could not be reached on the session bus"),
        )
    }

    fn channel_error(&self, channel: &str) -> LicomanError {
        LicomanError::new(
            ErrorKind::Unavailable,
            format!(
                "The Xfconf channel {channel:?} has no stored state on this system, so these \
                 settings are unavailable. The owning application records the channel once it \
                 saves a setting; Licoman does not invent values."
            ),
            format!("channel {channel:?} not in ListChannels"),
        )
    }

    /// Map a decoded value to the setting type the descriptor declares.
    /// Strings become `Choice` for enum settings (the dropdown matches on the
    /// stored value) and `Text` otherwise.
    fn to_setting_value(
        &self,
        value: XfconfValue,
        descriptor: &SettingDescriptor,
    ) -> Result<SettingValue, LicomanError> {
        Ok(match value {
            XfconfValue::Bool(b) => SettingValue::Bool(b),
            XfconfValue::Int(i) => SettingValue::Integer(i),
            XfconfValue::Double(d) => SettingValue::Float(d),
            XfconfValue::Str(s) => {
                if descriptor.setting_type == crate::value::SettingType::Choice {
                    SettingValue::Choice(s)
                } else {
                    SettingValue::Text(s)
                }
            }
            XfconfValue::StrList(items) => SettingValue::List(items),
        })
    }
}

impl SettingsReadBackend for XfconfReadBackend {
    fn kind(&self) -> &'static str {
        "xfconf"
    }

    fn probe(&self, binding: &BackendBinding) -> SupportState {
        let BackendBinding::Xfconf { channel, property } = binding else {
            return SupportState::Unavailable {
                reason: "Not an Xfconf binding.".into(),
            };
        };
        if !self
            .call_bounded("probe-locate", |client| client.locate())
            .unwrap_or(false)
        {
            return SupportState::Unavailable {
                reason: "The Xfconf configuration service (xfconfd) is not running. It is                          D-Bus-activatable, but Licoman never starts services to make a                          setting look available."
                    .into(),
            };
        }
        let channels = match self.call_bounded("probe-channels", |client| client.list_channels()) {
            Ok(channels) => channels,
            Err(ClientError::NoBus(_)) => {
                return SupportState::Unavailable {
                    reason: "The session message bus is not reachable.".into(),
                };
            }
            Err(ClientError::Timeout) => {
                return SupportState::Degraded {
                    note: "The Xfconf service did not answer in time; values may still be \
                           readable."
                        .into(),
                };
            }
            // The daemon answered but refused the read — degraded, not
            // absent (the absence shape is `Other` from the real client).
            Err(ClientError::Denied(_)) => {
                return SupportState::Degraded {
                    note: "The Xfconf service refused the channel query; values may still \
                           be readable."
                        .into(),
                };
            }
            Err(_) => {
                return SupportState::Unavailable {
                    reason: "The Xfconf configuration service (xfconfd) is not running.".into(),
                };
            }
        };
        if !channels.iter().any(|listed| listed == channel) {
            return SupportState::Unavailable {
                reason: format!(
                    "The Xfconf channel {channel:?} has no stored state on this system. The \
                     owning application records the channel once it saves a setting."
                ),
            };
        }
        // Clone for the bounded worker thread (it outlives the borrow).
        let probe_channel = channel.clone();
        let probe_property = property.clone();
        match self.call_bounded("probe-property", move |client| {
            client.property_exists(&probe_channel, &probe_property)
        }) {
            Ok(true) => SupportState::Available,
            Ok(false) => SupportState::Unavailable {
                reason: format!(
                    "The property {property:?} is not present in the channel {channel:?}. The \
                     application's built-in default applies; Licoman does not invent values."
                ),
            },
            Err(_) => SupportState::Degraded {
                note: "The channel exists but the property could not be checked right now.".into(),
            },
        }
    }

    fn read(
        &self,
        binding: &BackendBinding,
        descriptor: &SettingDescriptor,
    ) -> Result<ReadResult, LicomanError> {
        let BackendBinding::Xfconf { channel, property } = binding else {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "This setting is not bound to an Xfconf channel.",
                format!("xfconf backend got binding kind {:?}", binding.kind_name()),
            ));
        };
        let located = self
            .call_bounded("read-locate", |client| client.locate())
            .map_err(|error| match error {
                ClientError::NoBus(_) | ClientError::Other(_) => self.service_error(),
                other => other.to_licoman("locating the Xfconf service"),
            })?;
        if !located {
            return Err(self.service_error());
        }
        let channels = self
            .call_bounded("read-channels", |client| client.list_channels())
            .map_err(|error| match error {
                ClientError::NoBus(_) | ClientError::Other(_) => self.service_error(),
                other => other.to_licoman("ListChannels"),
            })?;
        if !channels.iter().any(|listed| listed == channel) {
            return Err(self.channel_error(channel));
        }
        // Clone for the bounded worker threads (they outlive the borrow).
        let read_channel = channel.clone();
        let read_property = property.clone();
        let exists = self
            .call_bounded("read-exists", move |client| {
                client.property_exists(&read_channel, &read_property)
            })
            .map_err(|error| error.to_licoman("PropertyExists"))?;
        if !exists {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "This property is not present in the Xfconf channel. The application's \
                 built-in default applies, but Licoman does not ship validated defaults."
                    .to_string(),
                format!("property {property:?} absent from channel {channel:?}"),
            ));
        }
        let read_channel = channel.clone();
        let read_property = property.clone();
        let value = self
            .call_bounded("read-property", move |client| {
                client.get_property(&read_channel, &read_property)
            })
            .map_err(|error| error.to_licoman("GetProperty"))?;
        let setting = self.to_setting_value(value, descriptor)?;
        let fingerprint_material = format!("{channel}{property}:{setting:?}");
        Ok(ReadResult {
            value: setting,
            origin: ValueOrigin::Runtime,
            writable: Writable::No {
                reason: "Licoman reads Xfconf channels but cannot change them yet. Writes are \
                         not enabled in this build; nothing was modified."
                    .to_string(),
            },
            freshness: Freshness::now(fnv1a_fingerprint(fingerprint_material.as_bytes())),
            restart_effect: match &descriptor.restart_target {
                Some(target) => RestartEffect::from_target(target),
                None => RestartEffect::None,
            },
            details: SourceDetails {
                backend_kind: "xfconf",
                target: format!(
                    "{XFCONF_SERVICE} {XFCONF_PATH} channel {channel:?} property \
                                {property:?}"
                ),
                origin_note: Some(format!(
                    "live value from {XFCONF_SERVICE} (xfconfd merges user overrides over \
                     package defaults)"
                )),
                imported: false,
                diagnostics: vec![format!("channel property count: {}", channels.len())],
            },
        })
    }

    fn watch_paths(&self) -> Vec<std::path::PathBuf> {
        // Property changes surface through the D-Bus signal watcher (the
        // adapter's listener thread), not the filesystem watcher.
        Vec::new()
    }
}

/// One decoded Xfconf change notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XfconfChange {
    pub channel: String,
    pub property: String,
}

/// Parse one Xfconf signal message into a change. Only `PropertyChanged`
/// (ssv) and `PropertyRemoved` (ss) on the Xfconf interface count; anything
/// else is filtered. Unit-testable without a bus (a locally built signal
/// message is the same wire object the watcher thread receives).
pub fn parse_change_message(message: &zbus::Message) -> Option<XfconfChange> {
    if message.header().interface()?.as_str() != XFCONF_INTERFACE
        || message.header().path()?.as_str() != XFCONF_PATH
    {
        return None;
    }
    match message.header().member()?.as_str() {
        "PropertyChanged" => {
            let (channel, property, _value): (String, String, OwnedValue) =
                message.body().deserialize().ok()?;
            Some(XfconfChange { channel, property })
        }
        "PropertyRemoved" => {
            let (channel, property): (String, String) = message.body().deserialize().ok()?;
            Some(XfconfChange { channel, property })
        }
        _ => None,
    }
}

/// Subscribe to the Xfconf change signals on a dedicated thread, forwarding
/// each notification to `notify` (the caller debounces, exactly like the
/// GSettings native watcher). The thread owns its connection; the returned
/// guard cancels it — the thread exits at its next wakeup, never joined
/// (see the `Drop` impl for why). Returns `None` when the session bus or the
/// signal subscription is unavailable — Refresh remains the invalidation
/// path, as with every watcher.
pub fn spawn_change_watcher(
    notify: impl Fn() + Send + Sync + 'static,
) -> Option<XfconfChangeWatch> {
    let notify = Arc::new(notify);
    let (cancel_tx, cancel_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let join = std::thread::Builder::new()
        .name("licoman-xfconf-watch".into())
        .spawn(move || {
            use zbus::message::Type;
            let run = || -> Result<(), zbus::Error> {
                let connection = zbus::blocking::Connection::session()?;
                let rule = zbus::MatchRule::builder()
                    .msg_type(Type::Signal)
                    .sender(XFCONF_SERVICE)?
                    .interface(XFCONF_INTERFACE)?
                    .build();
                let iterator =
                    zbus::blocking::MessageIterator::for_match_rule(rule, &connection, Some(64))?;
                for message in iterator {
                    if cancel_rx.try_recv().is_ok() {
                        break;
                    }
                    // A malformed/undecodable message is skipped — the
                    // subscription itself stays alive.
                    let Ok(message) = message else {
                        continue;
                    };
                    if parse_change_message(&message).is_some() {
                        notify();
                    }
                }
                Ok(())
            };
            // Errors here mean no bus / no daemon — the Refresh path covers
            // invalidation; log-worthy only.
            let _ = run();
        })
        .ok()?;
    Some(XfconfChangeWatch {
        cancel: cancel_tx,
        join: Some(join),
    })
}

/// Guard for the Xfconf signal watcher thread.
pub struct XfconfChangeWatch {
    cancel: std::sync::mpsc::SyncSender<()>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for XfconfChangeWatch {
    fn drop(&mut self) {
        let _ = self.cancel.send(());
        // Never join. The watcher blocks inside the zbus `MessageIterator`,
        // which only yields when xfconfd broadcasts a change signal, and the
        // cancel flag is only consulted between signals — a quiet bus parks
        // the thread indefinitely. Joining from the live run blocks the UI
        // thread: the drop happens during window teardown on the main
        // thread, where the hang keeps the Wayland event loop from answering
        // the compositor's pings and from completing the close request
        // (observed as the desktop's "application not responding" dialog on
        // close). The queued cancel makes the thread exit at its next
        // wakeup; a thread parked on a quiet bus is reaped when the process
        // exits. The guard is created once per process (AppState), so
        // detached watchers cannot accumulate.
        //
        // Dropping the handle detaches the thread.
        drop(self.join.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{Control, Scope};
    use crate::value::SettingType;
    use std::sync::Mutex;

    fn descriptor(id: &str, setting_type: SettingType) -> SettingDescriptor {
        SettingDescriptor::new(
            id,
            Scope::Desktop,
            "Test",
            "Test",
            "",
            setting_type,
            Control::ReadOnly,
        )
    }

    fn binding(channel: &str, property: &str) -> BackendBinding {
        BackendBinding::Xfconf {
            channel: channel.into(),
            property: property.into(),
        }
    }

    /// Mock client covering the required fixture matrix: absent service,
    /// live channels, missing channel, missing property, timeout, denial.
    struct Mock {
        state: Arc<Mutex<MockState>>,
    }

    #[derive(Default)]
    struct MockState {
        present: bool,
        fail_all: bool,
        denied: bool,
        channels: Vec<String>,
        properties: Vec<(String, XfconfValue)>,
    }

    impl Mock {
        fn absent() -> Self {
            Self {
                state: Arc::new(Mutex::new(MockState::default())),
            }
        }
        fn live() -> (Self, Arc<Mutex<MockState>>) {
            let state = Arc::new(Mutex::new(MockState {
                present: true,
                channels: vec!["thunar".into(), "xfwm4".into()],
                properties: vec![
                    ("/misc-single-click".into(), XfconfValue::Bool(true)),
                    (
                        "/misc-date-style".into(),
                        XfconfValue::Str("THUNAR_DATE_STYLE_SIMPLE".into()),
                    ),
                    ("/misc-separator-position".into(), XfconfValue::Int(170)),
                    ("/misc-ratio".into(), XfconfValue::Double(1.5)),
                    (
                        "/misc-list".into(),
                        XfconfValue::StrList(vec!["a".into(), "b".into()]),
                    ),
                ],
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

    impl XfconfClient for Mock {
        fn locate(&self) -> Result<bool, ClientError> {
            // A registered name answers ListNames regardless of whether the
            // daemon then denies or hangs on method calls — those states are
            // surfaced by the subsequent calls, not by absence.
            let state = self.state.lock().unwrap();
            Ok(state.present)
        }
        fn list_channels(&self) -> Result<Vec<String>, ClientError> {
            let state = self.state.lock().unwrap();
            if !state.present {
                return Err(ClientError::Other("service absent".into()));
            }
            if state.denied {
                return Err(ClientError::Denied("denied".into()));
            }
            if state.fail_all {
                return Err(ClientError::Timeout);
            }
            Ok(state.channels.clone())
        }
        fn get_property(&self, channel: &str, property: &str) -> Result<XfconfValue, ClientError> {
            let state = self.state.lock().unwrap();
            if !state.present {
                return Err(ClientError::Other("service absent".into()));
            }
            if state.fail_all {
                return Err(ClientError::Timeout);
            }
            if !state.channels.iter().any(|c| c == channel) {
                return Err(ClientError::Other(format!("no channel {channel}")));
            }
            state
                .properties
                .iter()
                .find(|(name, _)| name == property)
                .map(|(_, value)| value.clone())
                .ok_or_else(|| ClientError::Other(format!("no property {property}")))
        }
        fn property_exists(&self, channel: &str, property: &str) -> Result<bool, ClientError> {
            let state = self.state.lock().unwrap();
            if !state.present {
                return Err(ClientError::Other("service absent".into()));
            }
            if state.fail_all {
                return Err(ClientError::Timeout);
            }
            if !state.channels.iter().any(|c| c == channel) {
                return Err(ClientError::Other(format!("no channel {channel}")));
            }
            Ok(state.properties.iter().any(|(name, _)| name == property))
        }
    }

    #[test]
    fn absent_service_is_a_distinct_truthful_state() {
        let backend = XfconfReadBackend::with_client(Arc::new(Mock::absent()));
        let error = backend
            .read(
                &binding("thunar", "/misc-single-click"),
                &descriptor("x1", SettingType::Bool),
            )
            .unwrap_err();
        assert!(error.user_message.contains("xfconfd) is not running"));
        assert!(matches!(
            backend.probe(&binding("thunar", "/misc-single-click")),
            SupportState::Unavailable { .. }
        ));
    }

    #[test]
    fn unresponsive_service_times_out_without_hanging() {
        let (mock, state) = Mock::live();
        state.lock().unwrap().fail_all = true;
        let backend = XfconfReadBackend::with_client(Arc::new(mock));
        let started = std::time::Instant::now();
        let result = backend.probe(&binding("thunar", "/misc-single-click"));
        assert!(started.elapsed() < METHOD_TIMEOUT);
        assert!(matches!(result, SupportState::Degraded { .. }));
    }

    #[test]
    fn denied_reply_is_a_permission_error_on_read() {
        let (mock, state) = Mock::live();
        state.lock().unwrap().denied = true;
        let backend = XfconfReadBackend::with_client(Arc::new(mock));
        // A denied ListChannels is a permission error, distinct from absence
        // (the daemon answered — it refused). The probe takes the Degraded
        // path for the same reply.
        let error = backend
            .read(
                &binding("thunar", "/misc-single-click"),
                &descriptor("x2", SettingType::Bool),
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Permission);
        assert!(matches!(
            backend.probe(&binding("thunar", "/misc-single-click")),
            SupportState::Degraded { .. }
        ));
    }

    #[test]
    fn missing_channel_and_missing_property_are_distinct_states() {
        let (mock, _state) = Mock::live();
        let backend = XfconfReadBackend::with_client(Arc::new(mock));

        let error = backend
            .read(
                &binding("xfdesktop", "/misc-x"),
                &descriptor("x3", SettingType::Bool),
            )
            .unwrap_err();
        assert!(error.user_message.contains("no stored state"));

        let error = backend
            .read(
                &binding("thunar", "/misc-absent"),
                &descriptor("x4", SettingType::Bool),
            )
            .unwrap_err();
        assert!(
            error
                .user_message
                .contains("not present in the Xfconf channel")
        );

        // …and the probe distinguishes them too.
        assert!(matches!(
            backend.probe(&binding("xfdesktop", "/misc-x")),
            SupportState::Unavailable { .. }
        ));
        assert!(matches!(
            backend.probe(&binding("thunar", "/misc-absent")),
            SupportState::Unavailable { .. }
        ));
    }

    #[test]
    fn typed_values_decode_per_the_declared_type() {
        let (mock, _state) = Mock::live();
        let backend = XfconfReadBackend::with_client(Arc::new(mock));
        let read = |property: &str, kind: SettingType| {
            backend
                .read(&binding("thunar", property), &descriptor("x5", kind))
                .unwrap()
        };
        assert_eq!(
            read("/misc-single-click", SettingType::Bool).value,
            SettingValue::Bool(true)
        );
        assert_eq!(
            read("/misc-separator-position", SettingType::Integer).value,
            SettingValue::Integer(170)
        );
        assert_eq!(
            read("/misc-ratio", SettingType::Float).value,
            SettingValue::Float(1.5)
        );
        assert_eq!(
            read("/misc-list", SettingType::List).value,
            SettingValue::List(vec!["a".into(), "b".into()])
        );
        // A string becomes Choice for enum descriptors…
        let mut choice = descriptor("x6", SettingType::Choice);
        choice.restart_target = None;
        assert_eq!(
            read("/misc-date-style", SettingType::Choice).value,
            SettingValue::Choice("THUNAR_DATE_STYLE_SIMPLE".into())
        );
        // …and Text otherwise.
        let _ = &mut choice;
        assert_eq!(
            read("/misc-date-style", SettingType::Text).value,
            SettingValue::Text("THUNAR_DATE_STYLE_SIMPLE".into())
        );
    }

    #[test]
    fn reads_are_runtime_origin_and_explicitly_read_only() {
        let (mock, _state) = Mock::live();
        let backend = XfconfReadBackend::with_client(Arc::new(mock));
        let result = backend
            .read(
                &binding("thunar", "/misc-single-click"),
                &descriptor("x7", SettingType::Bool),
            )
            .unwrap();
        assert_eq!(result.origin, ValueOrigin::Runtime);
        match &result.writable {
            Writable::No { reason } => assert!(reason.contains("cannot change them yet")),
            other => panic!("expected Writable::No, got {other:?}"),
        }
        assert_eq!(result.details.backend_kind, "xfconf");
        assert!(result.details.target.contains("thunar"));
        assert!(
            result
                .details
                .origin_note
                .as_deref()
                .unwrap()
                .contains("xfconfd")
        );
    }

    #[test]
    fn non_xfconf_bindings_are_refused() {
        let (mock, _state) = Mock::live();
        let backend = XfconfReadBackend::with_client(Arc::new(mock));
        let other = BackendBinding::Mock;
        let error = backend
            .read(&other, &descriptor("x8", SettingType::Bool))
            .unwrap_err();
        assert!(
            error
                .user_message
                .contains("not bound to an Xfconf channel")
        );
    }

    #[test]
    fn change_signals_parse_from_locally_built_messages() {
        // PropertyChanged(ssv) on the Xfconf interface/path — the same wire
        // object the watcher thread receives from the bus.
        let message = zbus::Message::signal(XFCONF_PATH, XFCONF_INTERFACE, "PropertyChanged")
            .unwrap()
            .sender(":1.42")
            .unwrap()
            .build(&(
                "thunar",
                "/misc-single-click",
                zbus::zvariant::Value::Bool(false),
            ))
            .unwrap();
        let change = parse_change_message(&message).expect("parses");
        assert_eq!(change.channel, "thunar");
        assert_eq!(change.property, "/misc-single-click");

        // PropertyRemoved(ss).
        let message = zbus::Message::signal(XFCONF_PATH, XFCONF_INTERFACE, "PropertyRemoved")
            .unwrap()
            .sender(":1.42")
            .unwrap()
            .build(&("xfwm4", "/general/theme"))
            .unwrap();
        let change = parse_change_message(&message).expect("parses");
        assert_eq!(change.channel, "xfwm4");
        assert_eq!(change.property, "/general/theme");

        // A foreign signal is filtered out.
        let message = zbus::Message::signal(
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameOwnerChanged",
        )
        .unwrap()
        .build(&("com.example.Nobody", "", ""))
        .unwrap();
        assert!(parse_change_message(&message).is_none());
    }
    #[test]
    fn value_decode_rejects_unknown_shapes() {
        use zbus::zvariant::Value;
        let dict = Value::Dict(std::collections::HashMap::<String, Value>::new().into());
        let owned: OwnedValue = dict.try_into().unwrap();
        let error = decode_owned_value(owned).unwrap_err();
        assert!(error.to_string().contains("unsupported Xfconf value type"));
    }
}
