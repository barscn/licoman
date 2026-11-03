//! Phase 08 integration matrix: the two new backend families exercised at
//! the store level, not just in their unit suites — Xfconf (read,
//! explicit-read-only, watch/refresh wiring, safe failure paths) and the mpv
//! file resolution (read, version-gated write through the full engine,
//! external-change conflicts, malformed input). Fixtures only; nothing here
//! touches the host.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::error::ErrorKind;
use crate::modules::provider::ModuleProvider;
use crate::modules::registry::{ModuleRegistry, ModuleSource};
use crate::provider::Provider as _;
use crate::read::BackendRouter;
use crate::read::xfconf::{ClientError, XfconfClient, XfconfValue};
use crate::value::SettingValue;
use crate::write::{Change, ChangeIntent, FileWriteBackend, ServiceWriteBackend, WriteEngine};

// ---------------------------------------------------------------------------
// Xfconf fixtures
// ---------------------------------------------------------------------------

/// A controllable fake xfconfd: present/absent, channel/property states,
/// fail/deny switches — the same shape as the dbus tests' mock.
struct FakeXfconf {
    state: Arc<Mutex<FakeXfconfState>>,
}

#[derive(Default)]
struct FakeXfconfState {
    present: bool,
    fail_all: bool,
    channels: Vec<String>,
    properties: Vec<(String, XfconfValue)>,
}

impl FakeXfconf {
    fn absent() -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeXfconfState::default())),
        }
    }
    fn live() -> (Self, Arc<Mutex<FakeXfconfState>>) {
        let state = Arc::new(Mutex::new(FakeXfconfState {
            present: true,
            channels: vec!["thunar".into()],
            properties: vec![
                ("/misc-single-click".into(), XfconfValue::Bool(true)),
                (
                    "/misc-date-style".into(),
                    XfconfValue::Str("THUNAR_DATE_STYLE_SIMPLE".into()),
                ),
            ],
            ..FakeXfconfState::default()
        }));
        (
            Self {
                state: state.clone(),
            },
            state,
        )
    }
}

impl XfconfClient for FakeXfconf {
    fn locate(&self) -> Result<bool, ClientError> {
        let state = self.state.lock().unwrap();
        Ok(state.present && !state.fail_all)
    }
    fn list_channels(&self) -> Result<Vec<String>, ClientError> {
        let state = self.state.lock().unwrap();
        if !state.present || state.fail_all {
            return Err(ClientError::Other("service absent".into()));
        }
        Ok(state.channels.clone())
    }
    fn get_property(&self, channel: &str, property: &str) -> Result<XfconfValue, ClientError> {
        let state = self.state.lock().unwrap();
        if !state.present || state.fail_all {
            return Err(ClientError::Other("service absent".into()));
        }
        if !state.channels.iter().any(|c| c == channel) {
            return Err(ClientError::Other("no channel".into()));
        }
        state
            .properties
            .iter()
            .find(|(name, _)| name == property)
            .map(|(_, value)| value.clone())
            .ok_or_else(|| ClientError::Other("no property".into()))
    }
    fn property_exists(&self, channel: &str, property: &str) -> Result<bool, ClientError> {
        let state = self.state.lock().unwrap();
        if !state.present || state.fail_all {
            return Err(ClientError::Other("service absent".into()));
        }
        if !state.channels.iter().any(|c| c == channel) {
            return Err(ClientError::Other("no channel".into()));
        }
        Ok(state.properties.iter().any(|(name, _)| name == property))
    }
}

const XFCE_MODULE: &str = r#"
schema_version = 1
id = "desktop.xfce.thunar"
scope = "desktop"
title = "Thunar (Xfce)"
description = "File-manager behaviour read from the Xfconf channel."
category = "Xfce"
docs_url = "https://docs.xfce.org/xfce/thunar/preferences"
compatibility = { required_backends = ["xfconf"] }

[[settings]]
id = "single_click"
title = "Single click activates items"
type = "boolean"
control = "read_only"
policy = "read_only"
restart = "app"

[settings.backend]
kind = "xfconf"
channel = "thunar"
property = "/misc-single-click"

[[settings]]
id = "date_style"
title = "Date display style"
type = "enum"
control = "read_only"
policy = "read_only"
options = [
    { value = "THUNAR_DATE_STYLE_SIMPLE", label = "Simple" },
    { value = "THUNAR_DATE_STYLE_SHORT", label = "Short" },
    { value = "THUNAR_DATE_STYLE_LONG", label = "Long" },
    { value = "THUNAR_DATE_STYLE_ISO", label = "ISO 8601" },
]

[settings.backend]
kind = "xfconf"
channel = "thunar"
property = "/misc-date-style"
"#;

/// A fixture snapshot with the `xfconf` capability on the session bus (what
/// discovery emits when org.xfce.Xfconf is visible).
fn xfconf_snapshot() -> crate::discovery::EnvironmentSnapshot {
    let mut snapshot = crate::discovery::demo_fixtures::arch_hyprland_wayland();
    snapshot.bus_names.insert("org.xfce.Xfconf".to_string());
    snapshot
        .available_backends
        .push(crate::discovery::BackendCapability {
            id: "xfconf".to_string(),
            kind: crate::discovery::BackendKind::Service,
            status: crate::discovery::BackendStatus::Available,
            evidence: Vec::new(),
            detail: None,
        });
    snapshot
}

fn xfce_provider(client: Arc<dyn XfconfClient>) -> ModuleProvider {
    let registry = Arc::new(Mutex::new(ModuleRegistry::load([(
        ModuleSource::Builtin("desktop.xfce.thunar"),
        XFCE_MODULE.to_string(),
    )])));
    let router = BackendRouter::new(
        crate::read::gsettings::GSettingsReadBackend::for_paths(vec![], None, vec![]),
        crate::read::file::FileReadBackend::for_dirs(
            PathBuf::from("/nonexistent-config"),
            PathBuf::from("/nonexistent-home"),
            vec![],
        ),
        crate::read::dbus::DBusReadBackend::with_client(Arc::new(
            crate::read::dbus::AbsentServiceClient,
        )),
        crate::read::xfconf::XfconfReadBackend::with_client(client),
    );
    let provider = ModuleProvider::with_router(registry, router);
    // A fixture snapshot whose session bus carries the Xfconf name (the
    // capability the module requires).
    provider.set_environment(Some(&xfconf_snapshot()));
    provider
}

// ---------------------------------------------------------------------------
// mpv fixtures
// ---------------------------------------------------------------------------

const MPV_MODULE: &str = r#"
schema_version = 1
id = "app.mpv"
scope = "application"
title = "mpv"
description = "Player options stored in the documented mpv.conf."
category = "Media"
group = "Playback"
docs_url = "https://mpv.io/manual/master/#configuration-files"
compatibility = { required_backends = ["dbus-session"] }

[[settings]]
id = "fullscreen"
title = "Start in fullscreen"
type = "boolean"
control = "switch"
default = false

[settings.backend]
kind = "file"
path = "mpv/mpv.conf"
format = "mpv_conf"
resolution = "mpv"
key = "fullscreen"

[[settings]]
id = "keep_open"
title = "Keep the window open at the end"
type = "enum"
control = "dropdown"
default = "no"
options = [
    { value = "no" },
    { value = "yes" },
    { value = "always" },
]

[settings.backend]
kind = "file"
path = "mpv/mpv.conf"
format = "mpv_conf"
resolution = "mpv"
key = "keep-open"

[[settings]]
id = "save_position"
title = "Save position on quit"
type = "boolean"
control = "switch"
default = false

[settings.backend]
kind = "file"
path = "mpv/mpv.conf"
format = "mpv_conf"
resolution = "mpv"
key = "save-position-on-quit"
"#;

struct MpvFixture {
    dir: PathBuf,
    provider: ModuleProvider,
}

/// Unique per-call temp dir (tests run in parallel within one process).
fn temp_fixture_dir(tag: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("licoman-p08-{tag}-{}-{unique}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn mpv_provider(conf: &str) -> MpvFixture {
    let dir = temp_fixture_dir("mpv");
    let config = dir.join("config");
    std::fs::create_dir_all(config.join("mpv")).unwrap();
    std::fs::write(config.join("mpv/mpv.conf"), conf).unwrap();
    let registry = Arc::new(Mutex::new(ModuleRegistry::load([(
        ModuleSource::Builtin("app.mpv"),
        MPV_MODULE.to_string(),
    )])));
    let read =
        crate::read::file::FileReadBackend::for_dirs(config.clone(), dir.join("home"), vec![]);
    let router = BackendRouter::new(
        crate::read::gsettings::GSettingsReadBackend::for_paths(vec![], None, vec![]),
        read.clone(),
        crate::read::dbus::DBusReadBackend::with_client(Arc::new(
            crate::read::dbus::AbsentServiceClient,
        )),
        crate::read::xfconf::XfconfReadBackend::with_client(Arc::new(
            crate::read::xfconf::AbsentXfconfClient,
        )),
    );
    let engine = WriteEngine::new(
        FileWriteBackend::new(read, dir.join("backups")),
        ServiceWriteBackend::new(Arc::new(
            crate::write::service_write::FakeServiceClient::new(),
        )),
    );
    let provider = ModuleProvider::with_write_engine(registry, router, engine);
    provider.set_environment(Some(
        &crate::discovery::demo_fixtures::arch_hyprland_wayland(),
    ));
    MpvFixture { dir, provider }
}

impl Drop for MpvFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---------------------------------------------------------------------------
// Xfconf scenarios
// ---------------------------------------------------------------------------

#[test]
fn xfconf_reads_real_values_and_declares_explicit_read_only() {
    let (fake, state) = FakeXfconf::live();
    let provider = xfce_provider(Arc::new(fake));
    let result = provider
        .read_detailed("desktop.xfce.thunar.single_click")
        .unwrap();
    assert_eq!(result.value, SettingValue::Bool(true));
    assert_eq!(result.origin, crate::read::ValueOrigin::Runtime);
    match &result.writable {
        crate::read::Writable::No { reason } => {
            assert!(reason.contains("cannot change them yet"))
        }
        other => panic!("expected explicit read-only, got {other:?}"),
    }
    let detail = provider
        .read_detailed("desktop.xfce.thunar.date_style")
        .unwrap();
    assert_eq!(
        detail.value,
        SettingValue::Choice("THUNAR_DATE_STYLE_SIMPLE".into())
    );

    // External change (the daemon or a Thunar dialog flips the value) is
    // visible on the next read: same wiring the debounced refresh uses.
    state.lock().unwrap().properties[0].1 = XfconfValue::Bool(false);
    let refreshed = provider
        .read_detailed("desktop.xfce.thunar.single_click")
        .unwrap();
    assert_eq!(refreshed.value, SettingValue::Bool(false));
    assert_ne!(
        refreshed.freshness.fingerprint,
        result.freshness.fingerprint
    );
}

#[test]
fn xfconf_safe_failure_paths_are_distinct() {
    // Absent service (the module still applies — its capability matched a
    // fixture bus — but every row reports the honest service state).
    let provider = xfce_provider(Arc::new(FakeXfconf::absent()));
    let error = provider
        .read_detailed("desktop.xfce.thunar.single_click")
        .unwrap_err();
    assert!(error.user_message.contains("xfconfd) is not running"));

    // Unresponsive service: bounded, degraded — never a hang.
    let (fake, state) = FakeXfconf::live();
    state.lock().unwrap().fail_all = true;
    let provider = xfce_provider(Arc::new(fake));
    let error = provider
        .read_detailed("desktop.xfce.thunar.single_click")
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unavailable);

    // Absent channel/property on a live service: distinct row states.
    let (fake, state) = FakeXfconf::live();
    state.lock().unwrap().properties.clear();
    let provider = xfce_provider(Arc::new(fake));
    let error = provider
        .read_detailed("desktop.xfce.thunar.single_click")
        .unwrap_err();
    assert!(
        error
            .user_message
            .contains("not present in the Xfconf channel")
    );
}

#[test]
fn xfconf_module_applicability_requires_the_service_name() {
    // Without the xfconf capability the whole module is inapplicable with a
    // preserved backend reason.
    let registry = Arc::new(Mutex::new(ModuleRegistry::load([(
        ModuleSource::Builtin("desktop.xfce.thunar"),
        XFCE_MODULE.to_string(),
    )])));
    let provider = ModuleProvider::with_router(
        registry,
        crate::read::BackendRouter::new(
            crate::read::gsettings::GSettingsReadBackend::for_paths(vec![], None, vec![]),
            crate::read::file::FileReadBackend::for_dirs(
                PathBuf::from("/x"),
                PathBuf::from("/y"),
                vec![],
            ),
            crate::read::dbus::DBusReadBackend::with_client(Arc::new(
                crate::read::dbus::AbsentServiceClient,
            )),
            crate::read::xfconf::XfconfReadBackend::with_client(Arc::new(
                crate::read::xfconf::AbsentXfconfClient,
            )),
        ),
    );
    provider.set_environment(Some(
        &crate::discovery::demo_fixtures::arch_hyprland_wayland(),
    ));
    let error = provider
        .read("desktop.xfce.thunar.single_click")
        .unwrap_err();
    assert!(
        error.user_message.contains("xfconf"),
        "unexpected: {}",
        error.user_message
    );
}

#[test]
fn xfconf_writes_refuse_at_both_gates() {
    let (fake, _state) = FakeXfconf::live();
    let provider = xfce_provider(Arc::new(fake));
    // 1: the provider refuses before any request is built (the read_only
    // policy is the outermost gate here).
    let error = provider
        .write(
            "desktop.xfce.thunar.single_click",
            &SettingValue::Bool(false),
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::WriteFailed);
    assert!(
        error.user_message.contains("read-only"),
        "unexpected: {}",
        error.user_message
    );

    // 2: the engine refuses the typed read-only target (defense in depth).
    let engine = WriteEngine::new(
        FileWriteBackend::new(
            crate::read::file::FileReadBackend::for_dirs(
                PathBuf::from("/x"),
                PathBuf::from("/y"),
                vec![],
            ),
            PathBuf::from("/tmp/licoman-p08-backups"),
        ),
        ServiceWriteBackend::new(Arc::new(
            crate::write::service_write::FakeServiceClient::new(),
        )),
    );
    let registry = Arc::new(Mutex::new(ModuleRegistry::load([(
        ModuleSource::Builtin("desktop.xfce.thunar"),
        XFCE_MODULE.to_string(),
    )])));
    let provider2 = ModuleProvider::with_write_engine(
        registry,
        BackendRouter::new(
            crate::read::gsettings::GSettingsReadBackend::for_paths(vec![], None, vec![]),
            crate::read::file::FileReadBackend::for_dirs(
                PathBuf::from("/x"),
                PathBuf::from("/y"),
                vec![],
            ),
            crate::read::dbus::DBusReadBackend::with_client(Arc::new(
                crate::read::dbus::AbsentServiceClient,
            )),
            crate::read::xfconf::XfconfReadBackend::with_client(Arc::new(
                crate::read::xfconf::AbsentXfconfClient,
            )),
        ),
        engine,
    );
    provider2.set_environment(Some(&xfconf_snapshot()));
    let intent = ChangeIntent {
        setting_id: "desktop.xfce.thunar.single_click".into(),
        before: Some(SettingValue::Bool(true)),
        change: Change::Set(SettingValue::Bool(false)),
        expected_fingerprint: None,
    };
    let error = provider2.write_intent(&intent).unwrap_err();
    assert_eq!(error.kind, ErrorKind::WriteFailed);
    assert!(error.user_message.contains("cannot change them yet"));
}

// ---------------------------------------------------------------------------
// mpv scenarios
// ---------------------------------------------------------------------------

#[test]
fn mpv_reads_typed_values_from_the_documented_config() {
    let fixture =
        mpv_provider("# mpv conf\nfullscreen=yes\nkeep-open=always\n[myprofile]\nfullscreen=no\n");
    let provider = &fixture.provider;
    let fullscreen = provider.read_detailed("app.mpv.fullscreen").unwrap();
    assert_eq!(fullscreen.value, SettingValue::Bool(true));
    assert_eq!(fullscreen.origin, crate::read::ValueOrigin::User);
    assert_eq!(fullscreen.details.backend_kind, "file");

    let keep_open = provider.read_detailed("app.mpv.keep_open").unwrap();
    assert_eq!(keep_open.value, SettingValue::Choice("always".into()));

    // The absent top-level key is the honest default-applies state — even
    // though a profile-scoped `fullscreen=no` exists.
    let save = provider.read_detailed("app.mpv.save_position").unwrap_err();
    assert!(save.user_message.contains("not present"));
}

#[test]
fn mpv_writes_patch_the_minimum_line_and_preserve_everything_else() {
    let original = "# my mpv conf\nfullscreen=yes\nkeep-open=no\n\n[myprofile]\nfullscreen=no\n";
    let fixture = mpv_provider(original);
    let provider = &fixture.provider;
    let outcome = provider
        .write_intent(&ChangeIntent {
            setting_id: "app.mpv.fullscreen".into(),
            before: Some(SettingValue::Bool(true)),
            change: Change::Set(SettingValue::Bool(false)),
            expected_fingerprint: None,
        })
        .unwrap();
    assert!(outcome.outcome.verified);

    // The write landed (readback verified), the profile section survived.
    let conf = std::fs::read_to_string(fixture.dir.join("config/mpv/mpv.conf")).unwrap();
    assert!(conf.contains("# my mpv conf"));
    assert!(conf.contains("[myprofile]"));
    assert!(
        conf.starts_with("# my mpv conf\nfullscreen=no\nkeep-open=no\n"),
        "unexpected head: {conf:?}"
    );

    // An absent key is inserted before the first section header.
    provider
        .write_intent(&ChangeIntent {
            setting_id: "app.mpv.save_position".into(),
            before: None,
            change: Change::Set(SettingValue::Bool(true)),
            expected_fingerprint: None,
        })
        .unwrap();
    assert!(outcome.outcome.verified);
    let conf = std::fs::read_to_string(fixture.dir.join("config/mpv/mpv.conf")).unwrap();
    let save_line = conf
        .lines()
        .find(|l| l.starts_with("save-position-on-quit="));
    assert_eq!(save_line, Some("save-position-on-quit=yes"));
    // It sits before [myprofile].
    let save_index = conf
        .lines()
        .position(|l| l.starts_with("save-position-on-quit="));
    let section_index = conf.lines().position(|l| l == "[myprofile]");
    assert!(save_index < section_index);

    // Reset (RemoveOverride) deletes the line; the file stays valid.
    provider
        .write_intent(&ChangeIntent {
            setting_id: "app.mpv.save_position".into(),
            before: Some(SettingValue::Bool(true)),
            change: Change::RemoveOverride,
            expected_fingerprint: None,
        })
        .unwrap();
    assert!(outcome.outcome.verified);
    let conf = std::fs::read_to_string(fixture.dir.join("config/mpv/mpv.conf")).unwrap();
    assert!(!conf.contains("save-position-on-quit"));
}

#[test]
fn mpv_external_change_conflicts_and_writes_refuse_on_unknown_version() {
    let fixture = mpv_provider("fullscreen=yes\n");
    let provider = &fixture.provider;

    // External edit between read and apply → fingerprint conflict.
    let conf_path = fixture.dir.join("config/mpv/mpv.conf");
    let detailed = provider.read_detailed("app.mpv.fullscreen").unwrap();
    let fingerprint = detailed.freshness.fingerprint.clone();
    std::fs::write(&conf_path, "fullscreen=no\n").unwrap();
    let error = provider
        .write_intent(&ChangeIntent {
            setting_id: "app.mpv.fullscreen".into(),
            before: Some(SettingValue::Bool(true)),
            change: Change::Set(SettingValue::Bool(false)),
            expected_fingerprint: Some(fingerprint),
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Conflict);
    // The external value stands — Licoman never overwrites it.
    assert_eq!(
        std::fs::read_to_string(&conf_path).unwrap(),
        "fullscreen=no\n"
    );

    // Unknown mpv version: reads stay available, writes fail closed.
    let dir = temp_fixture_dir("mpv-unknown");
    let config = dir.join("config");
    std::fs::create_dir_all(config.join("mpv")).unwrap();
    std::fs::write(config.join("mpv/mpv.conf"), "fullscreen=yes\n").unwrap();
    let registry = Arc::new(Mutex::new(ModuleRegistry::load([(
        ModuleSource::Builtin("app.mpv"),
        MPV_MODULE.to_string(),
    )])));
    let read =
        crate::read::file::FileReadBackend::for_dirs(config.clone(), dir.join("home"), vec![])
            .with_mpv_support(crate::read::mpv::MpvSupport::Unknown);
    let router = BackendRouter::new(
        crate::read::gsettings::GSettingsReadBackend::for_paths(vec![], None, vec![]),
        read.clone(),
        crate::read::dbus::DBusReadBackend::with_client(Arc::new(
            crate::read::dbus::AbsentServiceClient,
        )),
        crate::read::xfconf::XfconfReadBackend::with_client(Arc::new(
            crate::read::xfconf::AbsentXfconfClient,
        )),
    );
    let engine = WriteEngine::new(
        FileWriteBackend::new(read, dir.join("backups")),
        ServiceWriteBackend::new(Arc::new(
            crate::write::service_write::FakeServiceClient::new(),
        )),
    );
    let unknown_provider = ModuleProvider::with_write_engine(registry, router, engine);
    unknown_provider.set_environment(Some(
        &crate::discovery::demo_fixtures::arch_hyprland_wayland(),
    ));
    let read = unknown_provider
        .read_detailed("app.mpv.fullscreen")
        .unwrap();
    assert_eq!(read.value, SettingValue::Bool(true));
    let intent = ChangeIntent {
        setting_id: "app.mpv.fullscreen".into(),
        before: Some(SettingValue::Bool(true)),
        change: Change::Set(SettingValue::Bool(false)),
        expected_fingerprint: None,
    };
    let error = unknown_provider.write_intent(&intent).unwrap_err();
    assert!(matches!(
        error.kind,
        ErrorKind::Permission | ErrorKind::WriteFailed
    ));
    let unknown_conf = dir.join("config/mpv/mpv.conf");
    assert_eq!(
        std::fs::read_to_string(&unknown_conf).unwrap(),
        "fullscreen=yes\n"
    );
}

#[test]
fn mpv_malformed_value_is_an_honest_read_error() {
    let fixture = mpv_provider("fullscreen=maybe\n");
    let error = fixture
        .provider
        .read_detailed("app.mpv.fullscreen")
        .unwrap_err();
    assert!(error.user_message.contains("type Licoman cannot read"));
}

#[test]
fn manifest_validation_accepts_the_new_bindings() {
    for (name, manifest) in [("xfconf", XFCE_MODULE), ("mpv", MPV_MODULE)] {
        let issues = crate::modules::manifest::validate_definition(
            &crate::modules::manifest::parse_manifest(manifest).unwrap(),
        );
        assert!(
            issues
                .iter()
                .all(|issue| issue.severity != crate::modules::manifest::IssueSeverity::Error),
            "{name} manifest issues: {issues:?}"
        );
    }
    // And the new grammar rules reject hostile payloads.
    let evil = XFCE_MODULE
        .replace(
            "property = \"/misc-single-click\"",
            "property = \"/x;rm-rf\"",
        )
        .replace("property = \"/misc-date-style\"", "property = \"/x;rm-rf\"");
    let parsed = crate::modules::manifest::parse_manifest(&evil).unwrap();
    let issues = crate::modules::manifest::validate_definition(&parsed);
    assert!(
        issues
            .iter()
            .any(|issue| issue.severity == crate::modules::manifest::IssueSeverity::Error)
    );
}
