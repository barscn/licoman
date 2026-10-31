//! Cross-backend test matrix (05_READ_ONLY_BACKENDS.md §6.4–6.6): all three
//! backend families loaded concurrently with one failing without affecting
//! the others, bounded timeouts that cannot stall the caller, proof that
//! read-only operation leaves fixture files unmodified, and proof that the
//! source details never carry configuration contents (no secrets).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::dbus::{ClientError, DBusReadBackend, PowerProfilesClient, PropertyValue};
use super::file::FileReadBackend;
use super::gsettings::GSettingsReadBackend;
use super::{BackendRouter, SettingsReadBackend};
use crate::descriptor::SettingDescriptor;
use crate::modules::def::{BackendBinding, FileFormat, FileResolution};

use crate::value::SettingValue;

fn fixtures() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn fixture_router() -> BackendRouter {
    let fixtures = fixtures();
    BackendRouter::new(
        GSettingsReadBackend::for_paths(
            vec![fixtures.join("discovery/schemas")],
            Some(fixtures.join("gsettings/dconf/user")),
            vec![fixtures.join("gsettings/dconf/system/site")],
        ),
        FileReadBackend::for_dirs(
            fixtures.join("alacritty/config"),
            fixtures.join("alacritty/home"),
            vec![fixtures.join("alacritty/etc")],
        ),
        DBusReadBackend::with_client(Arc::new(HangingClient)),
    )
}

/// A client whose property reads hang longer than the bounded timeout —
/// the "unresponsive service" fixture at the cross-backend level.
struct HangingClient;

impl PowerProfilesClient for HangingClient {
    fn locate(&self) -> Result<Option<(&'static str, &'static str, &'static str)>, ClientError> {
        Ok(Some((
            "org.freedesktop.UPower.PowerProfiles",
            "/org/freedesktop/UPower/PowerProfiles",
            "org.freedesktop.UPower.PowerProfiles",
        )))
    }
    fn property(
        &self,
        _endpoint: (&'static str, &'static str, &'static str),
        _name: &str,
    ) -> Result<PropertyValue, ClientError> {
        std::thread::sleep(super::dbus::METHOD_TIMEOUT + std::time::Duration::from_secs(2));
        Err(ClientError::Timeout)
    }
}

/// All three backend families read concurrently; one fails (unresponsive
/// service) without affecting the other two (§6.4).
#[test]
fn all_three_backends_read_concurrently_and_one_failure_is_isolated() {
    let router = Arc::new(fixture_router());
    let gsettings_binding = BackendBinding::GSettings {
        schema: "org.gnome.desktop.interface".into(),
        key: "clock-show-weekday".into(),
    };
    let file_binding = BackendBinding::File {
        path: "alacritty/alacritty.toml".into(),
        format: FileFormat::Toml,
        key: Some("font.size".into()),
        resolution: Some(FileResolution::Alacritty),
    };
    let dbus_binding = BackendBinding::DBus {
        service: "org.freedesktop.UPower.PowerProfiles".into(),
        path: "/org/freedesktop/UPower/PowerProfiles".into(),
        interface: "org.freedesktop.UPower.PowerProfiles".into(),
        property: Some("ActiveProfile".into()),
    };

    let started = std::time::Instant::now();
    let handles: Vec<_> = [
        ("gsettings", gsettings_binding),
        ("file", file_binding),
        ("dbus", dbus_binding),
    ]
    .into_iter()
    .map(|(name, binding)| {
        let router = router.clone();
        std::thread::spawn(move || {
            let descriptor = SettingDescriptor::new(
                format!("test.{name}"),
                crate::descriptor::Scope::Shared,
                "Test",
                "Test",
                "",
                crate::value::SettingType::Bool,
                crate::descriptor::Control::Switch,
            );
            (name, router.read(&binding, &descriptor))
        })
    })
    .collect();
    let mut results = BTreeMap::new();
    for handle in handles {
        let (name, result) = handle.join().expect("thread");
        results.insert(name, result);
    }
    let elapsed = started.elapsed();

    // The healthy backends read fine…
    let gsettings = results.get("gsettings").expect("gsettings result");
    assert!(gsettings.is_ok(), "gsettings read failed: {gsettings:?}");
    let file = results.get("file").expect("file result");
    assert!(file.is_ok(), "file read failed: {file:?}");
    // …and the unresponsive service fails with a bounded timeout.
    let dbus = results.get("dbus").expect("dbus result");
    let error = dbus.as_ref().unwrap_err();
    assert!(error.user_message.contains("did not answer in time"));
    // Bounded: one hanging service cannot dominate the pass.
    assert!(
        elapsed < super::dbus::METHOD_TIMEOUT + std::time::Duration::from_secs(2),
        "the read pass took {elapsed:?}; the timeout did not bound it"
    );
}

/// Reading fixture files leaves them byte-identical (§6.6: hash
/// before/after; no real host files are written).
#[test]
fn reads_leave_fixture_files_unmodified() {
    let hash_tree = |root: &std::path::Path| -> BTreeMap<String, u64> {
        let mut map = BTreeMap::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(bytes) = std::fs::read(&path) {
                    map.insert(path.display().to_string(), crate::read::fnv1a(&bytes));
                }
            }
        }
        map
    };

    let gsettings_root = fixtures().join("gsettings");
    let alacritty_root = fixtures().join("alacritty");
    let before_gsettings = hash_tree(&gsettings_root);
    let before_alacritty = hash_tree(&alacritty_root);

    // A full read pass over every backend.
    let router = fixture_router();
    let gsettings_binding = BackendBinding::GSettings {
        schema: "org.gnome.desktop.interface".into(),
        key: "clock-show-weekday".into(),
    };
    let file_binding = BackendBinding::File {
        path: "alacritty/alacritty.toml".into(),
        format: FileFormat::Toml,
        key: Some("font.size".into()),
        resolution: Some(FileResolution::Alacritty),
    };
    let descriptor = SettingDescriptor::new(
        "t",
        crate::descriptor::Scope::Shared,
        "T",
        "T",
        "",
        crate::value::SettingType::Bool,
        crate::descriptor::Control::Switch,
    );
    let _ = router.read(&gsettings_binding, &descriptor);
    let _ = router.read(&file_binding, &descriptor);

    assert_eq!(before_gsettings, hash_tree(&gsettings_root));
    assert_eq!(before_alacritty, hash_tree(&alacritty_root));
}

/// A secret value planted in a captured config fixture never appears in
/// the source details or diagnostics (§6.5).
#[test]
fn source_details_carry_no_config_contents() {
    let dir = std::env::temp_dir().join(format!("licoman-secrets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("config/alacritty")).unwrap();
    let secret = "SUPER-SECRET-TOKEN-9f13c";
    std::fs::write(
        dir.join("config/alacritty/alacritty.toml"),
        format!("font.size = 11.0\n[env]\nTOKEN = \"{secret}\"\n"),
    )
    .unwrap();
    let backend =
        FileReadBackend::for_dirs(dir.join("config"), dir.join("home"), vec![dir.join("etc")]);
    let binding = BackendBinding::File {
        path: "alacritty/alacritty.toml".into(),
        format: FileFormat::Toml,
        key: Some("font.size".into()),
        resolution: Some(FileResolution::Alacritty),
    };
    let descriptor = SettingDescriptor::new(
        "s",
        crate::descriptor::Scope::Application,
        "T",
        "T",
        "",
        crate::value::SettingType::Float,
        crate::descriptor::Control::Slider {
            step: 0.5,
            unit: String::new(),
        },
    );
    let result =
        <FileReadBackend as SettingsReadBackend>::read(&backend, &binding, &descriptor).unwrap();
    assert_eq!(result.value, SettingValue::Float(11.0));
    let details_text = format!("{}{:?}", result.details.target, result.details.diagnostics);
    assert!(
        !details_text.contains(secret),
        "source details leaked config contents: {details_text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A provider-level pass mirrors what the adapter does: background rows
/// start Unknown, reads publish per-row, and one row's failure leaves the
/// others Idle (§6.4 at the store level).
#[test]
fn store_level_background_pass_isolates_failures() {
    const MODULE: &str = r#"
schema_version = 1
id = "shared.cross"
scope = "shared"
title = "Cross"
category = "Test"

[[settings]]
id = "from_gsettings"
title = "From GSettings"
type = "boolean"
control = "switch"
default = true

[settings.backend]
kind = "gsettings"
schema = "org.gnome.desktop.interface"
key = "clock-show-weekday"

[[settings]]
id = "from_service"
title = "From service"
type = "enum"
control = "dropdown"
default = "balanced"
[[settings.options]]
value = "balanced"
[[settings.options]]
value = "performance"
[[settings.options]]
value = "power-saver"

[settings.backend]
kind = "dbus"
service = "org.freedesktop.UPower.PowerProfiles"
path = "/org/freedesktop/UPower/PowerProfiles"
interface = "org.freedesktop.UPower.PowerProfiles"
property = "ActiveProfile"
"#;
    let registry = crate::modules::ModuleRegistry::load([(
        crate::modules::ModuleSource::Builtin("m"),
        MODULE.to_string(),
    )]);
    let router = fixture_router();
    let provider =
        crate::modules::ModuleProvider::with_router(Arc::new(Mutex::new(registry)), router);
    provider.set_environment(Some(
        &crate::discovery::demo_fixtures::arch_hyprland_wayland(),
    ));

    let mut store = crate::model::Store::new(
        {
            let mut registry = crate::registry::Registry::new();
            registry.register(Box::new(provider.clone()));
            registry
        },
        crate::prefs::parse_preferences(""),
        std::env::temp_dir().join("licoman-cross-prefs.toml"),
    );

    let gsettings_id = "shared.cross.from_gsettings";
    let dbus_id = "shared.cross.from_service";
    assert_eq!(
        store.model(gsettings_id).unwrap().phase,
        crate::state::SettingPhase::Unknown
    );
    assert_eq!(
        store.model(dbus_id).unwrap().phase,
        crate::state::SettingPhase::Unknown
    );

    // Publish each row like the adapter's background pass does.
    let read = crate::provider::Provider::read_detailed(&provider, gsettings_id);
    assert!(store.publish_read(gsettings_id, read));
    let read = crate::provider::Provider::read_detailed(&provider, dbus_id);
    let dbus_error_text = format!("{:?}", read.as_ref().err());
    store.publish_read(dbus_id, read);

    // One failure does not affect the other.
    assert_eq!(
        store.model(gsettings_id).unwrap().phase,
        crate::state::SettingPhase::Idle
    );
    assert_eq!(
        store.model(gsettings_id).unwrap().stored,
        Some(SettingValue::Bool(true))
    );
    assert_eq!(
        store.model(dbus_id).unwrap().phase,
        crate::state::SettingPhase::Unavailable
    );
    assert!(dbus_error_text.contains("did not answer in time"));
}
