//! Phase 07 matrix (07_FIRST_REAL_INTEGRATIONS.md): the live-write gates
//! that go beyond Phase 06 — Alacritty version validation (D50) and the
//! xattr/ACL preserve-or-refuse rule (D53). Everything here runs on injected
//! fixture roots; nothing touches a real user configuration.

use crate::descriptor::{Control, Scope};
use crate::error::ErrorKind;
use crate::modules::def::{BackendBinding, RiskLevel};
use crate::modules::def::{FileFormat, FileResolution};
use crate::read::alacritty::{AlacrittySupport, AppVersion};
use crate::read::file::FileReadBackend;
use crate::value::{SettingType, SettingValue};
use crate::write::file_write::{FileWriteBackend, XattrOps};
use crate::write::{Change, ChangeIntent, ChangeRequest};
use std::path::PathBuf;

fn descriptor(id: &str, setting_type: SettingType) -> crate::descriptor::SettingDescriptor {
    let mut descriptor = crate::descriptor::SettingDescriptor::new(
        id,
        Scope::Application,
        "Test",
        "Test",
        "",
        setting_type,
        Control::Slider {
            step: 0.5,
            unit: String::new(),
        },
    );
    descriptor.min = Some(0.0);
    descriptor.max = Some(100.0);
    descriptor
}

fn alacritty_binding(key: &str) -> BackendBinding {
    BackendBinding::File {
        path: "alacritty/alacritty.toml".into(),
        format: FileFormat::Toml,
        key: Some(key.into()),
        resolution: Some(FileResolution::Alacritty),
    }
}

struct Fixture {
    dir: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fixture(files: &[(&str, &str)]) -> Fixture {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("licoman-p07-{}-{unique}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, content) in files {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, content).unwrap();
    }
    Fixture { dir }
}

fn read_backend(fx: &Fixture, support: AlacrittySupport) -> FileReadBackend {
    FileReadBackend::for_dirs(
        fx.dir.join("config"),
        fx.dir.join("home"),
        vec![fx.dir.join("etc")],
    )
    .with_alacritty_support(support)
}

fn write_backend(read: FileReadBackend) -> FileWriteBackend {
    FileWriteBackend::new(read, std::env::temp_dir().join("licoman-p07-backups"))
}

fn request(
    id: &str,
    binding: &BackendBinding,
    before: SettingValue,
    value: SettingValue,
) -> ChangeRequest {
    ChangeRequest::resolve(
        &descriptor(id, SettingType::Float),
        binding,
        &ChangeIntent {
            setting_id: id.to_string(),
            before: Some(before),
            change: Change::Set(value),
            expected_fingerprint: None,
        },
        RiskLevel::Low,
    )
}

// ---- D50: version-validated exposure --------------------------------------

#[test]
fn unknown_version_disables_writes_while_reads_stay_available() {
    let fx = fixture(&[(
        "config/alacritty/alacritty.toml",
        "font.size = 11.0\n# keep this comment\n",
    )]);
    let read = read_backend(&fx, AlacrittySupport::Unknown);
    let binding = alacritty_binding("font.size");
    let descriptor = descriptor("t1", SettingType::Float);

    // Reads stay available: the version gates writes, not knowledge.
    let result = crate::read::SettingsReadBackend::read(&read, &binding, &descriptor).unwrap();
    assert_eq!(result.value, SettingValue::Float(11.0));
    assert!(
        result
            .details
            .diagnostics
            .iter()
            .any(|d| d.contains("could not be verified"))
    );

    // Write support is refused with the honest reason — and the engine
    // refuses independently.
    assert!(matches!(
        read.write_support(&binding),
        crate::read::SupportState::Unavailable { .. }
    ));
    let engine_write = write_backend(read);
    let error = engine_write
        .apply(
            &request(
                "t1",
                &binding,
                SettingValue::Float(11.0),
                SettingValue::Float(14.0),
            ),
            &descriptor,
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Permission);
    assert!(error.user_message.contains("could not be verified"));
    // The file is untouched.
    let after = std::fs::read_to_string(fx.dir.join("config/alacritty/alacritty.toml")).unwrap();
    assert_eq!(after, "font.size = 11.0\n# keep this comment\n");
}

#[test]
fn legacy_yaml_only_configuration_is_a_distinct_unavailable_state() {
    let fx = fixture(&[("config/alacritty/alacritty.yml", "font:\n  size: 11.0\n")]);
    let read = read_backend(&fx, AlacrittySupport::LegacyYaml(AppVersion::new(0, 12, 9)));
    let binding = alacritty_binding("font.size");
    let descriptor = descriptor("t2", SettingType::Float);
    let error = crate::read::SettingsReadBackend::read(&read, &binding, &descriptor).unwrap_err();
    assert!(error.user_message.contains("legacy YAML"));
    assert!(error.user_message.contains("nothing was changed"));
    // Writes refuse as well, with the version verdict in the message.
    let error = write_backend(read)
        .apply(
            &request(
                "t2",
                &binding,
                SettingValue::Float(11.0),
                SettingValue::Float(14.0),
            ),
            &descriptor,
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Permission);
    assert!(error.user_message.contains("legacy YAML"));
}

#[test]
fn toml_era_version_writes_normally_and_preserves_the_file() {
    let original = "font.size = 11.0\n# keep this comment\n";
    let fx = fixture(&[("config/alacritty/alacritty.toml", original)]);
    let read = read_backend(&fx, AlacrittySupport::Toml(AppVersion::new(0, 13, 0)));
    let binding = alacritty_binding("font.size");
    let descriptor = descriptor("t3", SettingType::Float);
    assert!(matches!(
        read.write_support(&binding),
        crate::read::SupportState::Available
    ));
    let outcome = write_backend(read)
        .apply(
            &request(
                "t3",
                &binding,
                SettingValue::Float(11.0),
                SettingValue::Float(14.0),
            ),
            &descriptor,
        )
        .unwrap();
    assert!(outcome.verified);
    let after = std::fs::read_to_string(fx.dir.join("config/alacritty/alacritty.toml")).unwrap();
    assert!(after.contains("font.size = 14.0"));
    assert!(after.contains("# keep this comment"));
}

// ---- D53: xattr/ACL preserve-or-refuse ------------------------------------

/// A wrapper that fails on demand while delegating the real work otherwise.
struct FaultyXattrOps {
    fail_collect: bool,
    fail_restore: bool,
}

impl XattrOps for FaultyXattrOps {
    fn collect(
        &self,
        path: &std::path::Path,
    ) -> std::io::Result<Vec<(std::ffi::OsString, Vec<u8>)>> {
        if self.fail_collect {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "test fault: collect",
            ));
        }
        crate::write::file_write::RealXattrOps.collect(path)
    }

    fn restore(
        &self,
        path: &std::path::Path,
        attrs: &[(std::ffi::OsString, Vec<u8>)],
    ) -> std::io::Result<()> {
        if self.fail_restore {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "test fault: restore",
            ));
        }
        crate::write::file_write::RealXattrOps.restore(path, attrs)
    }
}

#[test]
fn user_xattrs_survive_a_write() {
    let original = "font.size = 11.0\n";
    let fx = fixture(&[("config/alacritty/alacritty.toml", original)]);
    let target = fx.dir.join("config/alacritty/alacritty.toml");
    xattr::set(&target, "user.licoman-test", b"kept").unwrap();
    let read = read_backend(&fx, AlacrittySupport::Toml(AppVersion::new(0, 13, 0)));
    let binding = alacritty_binding("font.size");
    let descriptor = descriptor("t4", SettingType::Float);
    let outcome = write_backend(read)
        .apply(
            &request(
                "t4",
                &binding,
                SettingValue::Float(11.0),
                SettingValue::Float(14.0),
            ),
            &descriptor,
        )
        .unwrap();
    assert!(outcome.verified);
    assert_eq!(
        xattr::get(&target, "user.licoman-test").unwrap().as_deref(),
        Some(&b"kept"[..]),
        "the extended attribute must survive the replacement"
    );
}

#[test]
fn unreadable_xattrs_refuse_the_write_up_front() {
    let original = "font.size = 11.0\n";
    let fx = fixture(&[("config/alacritty/alacritty.toml", original)]);
    let read = read_backend(&fx, AlacrittySupport::Toml(AppVersion::new(0, 13, 0)));
    let binding = alacritty_binding("font.size");
    let descriptor = descriptor("t5", SettingType::Float);
    let backend = write_backend(read).with_xattr_ops(Box::new(FaultyXattrOps {
        fail_collect: true,
        fail_restore: false,
    }));
    let error = backend
        .apply(
            &request(
                "t5",
                &binding,
                SettingValue::Float(11.0),
                SettingValue::Float(14.0),
            ),
            &descriptor,
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::WriteFailed);
    assert!(error.user_message.contains("extended attributes"));
    let after = std::fs::read_to_string(fx.dir.join("config/alacritty/alacritty.toml")).unwrap();
    assert_eq!(after, original, "nothing was modified");
}

#[test]
fn unrestorable_xattrs_refuse_the_write_and_leave_no_debris() {
    let original = "font.size = 11.0\n";
    let fx = fixture(&[("config/alacritty/alacritty.toml", original)]);
    let read = read_backend(&fx, AlacrittySupport::Toml(AppVersion::new(0, 13, 0)));
    let binding = alacritty_binding("font.size");
    let descriptor = descriptor("t6", SettingType::Float);
    let backend = write_backend(read).with_xattr_ops(Box::new(FaultyXattrOps {
        fail_collect: false,
        fail_restore: true,
    }));
    let error = backend
        .apply(
            &request(
                "t6",
                &binding,
                SettingValue::Float(11.0),
                SettingValue::Float(14.0),
            ),
            &descriptor,
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::WriteFailed);
    assert!(error.user_message.contains("extended attributes"));
    let after = std::fs::read_to_string(fx.dir.join("config/alacritty/alacritty.toml")).unwrap();
    assert_eq!(after, original, "nothing was modified");
    // No staging debris.
    let parent = fx.dir.join("config/alacritty");
    let debris: Vec<_> = std::fs::read_dir(&parent)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            name.starts_with('.') && name.contains(".licoman-") && name.ends_with(".tmp")
        })
        .collect();
    assert!(debris.is_empty(), "temp files must be swept: {debris:?}");
}

// ---- D51: provider-level gsettings gate (store-level flow) ----------------

#[test]
fn gsettings_writability_follows_the_desktop_gate_and_the_store_applies_natively() {
    use crate::modules::registry::ModuleSource;
    use std::sync::{Arc, Mutex};

    const MODULE: &str = r#"
schema_version = 1
id = "desktop.gate"
scope = "desktop"
title = "Gate"
category = "Test"

[[settings]]
id = "weekday"
title = "Weekday"
type = "boolean"
control = "switch"
policy = "immediate"
default = true

[settings.backend]
kind = "gsettings"
schema = "org.gnome.desktop.interface"
key = "clock-show-weekday"
"#;

    struct MemoryNativeFixture {
        backend: gio::SettingsBackend,
        source: gio::SettingsSchemaSource,
    }

    impl MemoryNativeFixture {
        fn fixture() -> Self {
            let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/discovery/schemas");
            Self {
                backend: gio::memory_settings_backend_new(),
                source: gio::SettingsSchemaSource::from_directory(&dir, None, true).unwrap(),
            }
        }
    }

    // SAFETY: thread-safe GLib objects used through shared references only
    // (see the same rationale in the gsettings_write suite).
    unsafe impl Send for MemoryNativeFixture {}
    unsafe impl Sync for MemoryNativeFixture {}

    impl crate::write::NativeGSettings for MemoryNativeFixture {
        fn read(
            &self,
            schema: &str,
            key: &str,
        ) -> Result<
            (SettingValue, Option<SettingValue>),
            crate::write::gsettings_write::GsClientError,
        > {
            use gio::prelude::*;
            let schema_obj = self.source.lookup(schema, true).ok_or(
                crate::write::gsettings_write::GsClientError::NoSchemaOrKey(schema.into()),
            )?;
            let settings = gio::Settings::new_full(
                &schema_obj,
                Some(&self.backend),
                schema_obj.path().as_deref(),
            );
            let to_value = |variant: gio::glib::Variant| {
                if variant.type_().as_str() == "b" {
                    SettingValue::Bool(variant.get::<bool>().unwrap_or_default())
                } else {
                    SettingValue::Text(variant.get::<String>().unwrap_or_default())
                }
            };
            let effective = to_value(settings.value(key));
            let user = settings.user_value(key).map(to_value);
            Ok((effective, user))
        }

        fn write(
            &self,
            schema: &str,
            key: &str,
            value: &SettingValue,
        ) -> Result<(), crate::write::gsettings_write::GsClientError> {
            use gio::glib::ToVariant as _;
            use gio::prelude::*;
            let schema_obj = self.source.lookup(schema, true).ok_or(
                crate::write::gsettings_write::GsClientError::NoSchemaOrKey(schema.into()),
            )?;
            let settings = gio::Settings::new_full(
                &schema_obj,
                Some(&self.backend),
                schema_obj.path().as_deref(),
            );
            let SettingValue::Bool(b) = value else {
                return Err(crate::write::gsettings_write::GsClientError::ValueRejected(
                    "fixture only supports bools".into(),
                ));
            };
            settings.set_value(key, &b.to_variant()).map_err(|error| {
                crate::write::gsettings_write::GsClientError::Denied(error.to_string())
            })
        }

        fn reset(
            &self,
            schema: &str,
            key: &str,
        ) -> Result<(), crate::write::gsettings_write::GsClientError> {
            use gio::prelude::*;
            let schema_obj = self.source.lookup(schema, true).ok_or(
                crate::write::gsettings_write::GsClientError::NoSchemaOrKey(schema.into()),
            )?;
            let settings = gio::Settings::new_full(
                &schema_obj,
                Some(&self.backend),
                schema_obj.path().as_deref(),
            );
            settings.reset(key);
            Ok(())
        }
    }

    struct AbsentService;
    impl crate::read::dbus::PowerProfilesClient for AbsentService {
        fn locate(
            &self,
        ) -> Result<Option<crate::read::dbus::Endpoint>, crate::read::dbus::ClientError> {
            Ok(None)
        }
        fn property(
            &self,
            _endpoint: crate::read::dbus::Endpoint,
            _name: &str,
        ) -> Result<crate::read::dbus::PropertyValue, crate::read::dbus::ClientError> {
            Err(crate::read::dbus::ClientError::Other("absent".into()))
        }
    }

    fn gated_provider(
        snapshot: &crate::discovery::EnvironmentSnapshot,
    ) -> crate::modules::ModuleProvider {
        let registry = Arc::new(Mutex::new(crate::modules::ModuleRegistry::load([(
            ModuleSource::Builtin("gate"),
            MODULE.to_string(),
        )])));
        let fx_dir = std::env::temp_dir().join(format!("licoman-p07-gate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&fx_dir);
        std::fs::create_dir_all(fx_dir.join("config")).unwrap();
        let router = crate::read::BackendRouter::new(
            // Reads through the committed fixture schema database (Phase 05
            // GVDB path — no real dconf anywhere).
            crate::read::gsettings::GSettingsReadBackend::for_paths(
                vec![
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../../tests/fixtures/discovery/schemas"),
                ],
                None,
                vec![],
            ),
            crate::read::file::FileReadBackend::for_dirs(
                fx_dir.join("config"),
                fx_dir.join("home"),
                vec![],
            ),
            crate::read::dbus::DBusReadBackend::with_client(Arc::new(AbsentService)),
            crate::read::xfconf::XfconfReadBackend::with_client(Arc::new(
                crate::read::xfconf::AbsentXfconfClient,
            )),
        );
        // The gate starts closed; `set_environment` drives it from the
        // detected session (the provider does this on every discovery ingest).
        let gsettings = Arc::new(crate::write::GSettingsWriteBackend::with_native(
            crate::write::GSettingsDesktopGate::closed(),
            Arc::new(MemoryNativeFixture::fixture()),
        ));
        let engine = crate::write::WriteEngine::new(
            crate::write::FileWriteBackend::new(
                crate::read::file::FileReadBackend::for_dirs(
                    fx_dir.join("config"),
                    fx_dir.join("home"),
                    vec![],
                ),
                fx_dir.join("backups"),
            ),
            crate::write::ServiceWriteBackend::new(Arc::new(
                crate::write::service_write::FakeServiceClient::new(),
            )),
        )
        .with_gsettings(gsettings);
        let provider = crate::modules::ModuleProvider::with_write_engine(registry, router, engine);
        provider.set_environment(Some(snapshot));
        provider
    }

    let id = "desktop.gate.weekday";

    // Non-GNOME session (the snapshot decides): the row is honestly
    // read-only even though the schema is present.
    let provider = gated_provider(&crate::discovery::demo_fixtures::arch_hyprland_wayland());
    let result = crate::provider::Provider::read_detailed(&provider, id).unwrap();
    assert!(matches!(result.writable, crate::read::Writable::No { .. }));

    // GNOME session: writable, and a store-level apply runs the native
    // write with the native verification (no false conflict from the
    // separate read transport).
    let provider = gated_provider(&crate::discovery::demo_fixtures::fedora_gnome_wayland());
    let result = crate::provider::Provider::read_detailed(&provider, id).unwrap();
    assert_eq!(result.writable, crate::read::Writable::Yes);

    // The native layer starts at the schema default (false).
    let intent = ChangeIntent {
        setting_id: id.to_string(),
        before: Some(SettingValue::Bool(false)),
        change: Change::Set(SettingValue::Bool(true)),
        expected_fingerprint: None,
    };
    let outcome = crate::provider::Provider::write_intent(&provider, &intent).unwrap();
    assert!(outcome.outcome.verified);
    let verification = outcome.verification.expect("verified writes carry a read");
    assert_eq!(verification.value, SettingValue::Bool(true));
    assert!(
        verification
            .details
            .origin_note
            .as_deref()
            .unwrap()
            .contains("native GSettings API")
    );
    // The reset path is the platform's real reset semantics.
    let reset =
        crate::provider::Provider::reset_support(&provider, id).expect("gsettings reset supported");
    assert!(matches!(
        reset.disclosure,
        crate::write::ResetDisclosure::DeletesOverride { .. }
    ));
    assert!(matches!(reset.kind, Change::RemoveOverride));
}
