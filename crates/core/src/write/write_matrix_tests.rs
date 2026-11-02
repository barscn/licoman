//! The adversarial write matrix (06_SAFE_WRITE_ENGINE.md §7): deterministic
//! tests over temp fixture roots, a fake D-Bus service and injected
//! failures. **No test ever touches the real `$HOME` or `/etc`.**

use std::path::PathBuf;
use std::sync::Arc;

use crate::descriptor::{ApplyMode, Control, Scope, SettingDescriptor};
use crate::error::ErrorKind;
use crate::modules::def::{FileFormat, FileResolution, RiskLevel};
use crate::read::dbus::PropertyValue;
use crate::read::file::FileReadBackend;
use crate::state::SettingPhase;
use crate::value::SettingValue;
use crate::write::file_write::{FaultSet, FileWriteBackend};
use crate::write::request::{Change, ChangeIntent, ServiceBus, ServiceEndpoint};
use crate::write::service_write::{FakeServiceClient, ServiceWriteBackend};
use crate::write::{ChangeRequest, DenyAllBroker, Persistence, WriteEngine, WritePolicy};

fn descriptor(id: &str, setting_type: crate::value::SettingType) -> SettingDescriptor {
    let mut d = SettingDescriptor::new(
        id,
        Scope::Application,
        "Test",
        "Test",
        "",
        setting_type,
        Control::Text {
            placeholder: String::new(),
        },
    );
    d.apply_mode = ApplyMode::Staged;
    d.module_id = "app.test".into();
    d
}

fn file_binding(
    path: &str,
    key: &str,
    resolution: Option<FileResolution>,
) -> crate::modules::def::BackendBinding {
    crate::modules::def::BackendBinding::File {
        path: path.into(),
        format: FileFormat::Toml,
        key: Some(key.into()),
        resolution,
    }
}

/// One isolated fixture environment: a temp config root (as `config_home`),
/// a backup root, and a file write backend over them. Dropped cleanly.
struct FileFixture {
    dir: PathBuf,
    write: FileWriteBackend,
}

impl Drop for FileFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl FileFixture {
    fn new(files: &[(&str, &str)]) -> Self {
        Self::with_uid(files, Box::new(crate::write::file_write::current_uid))
    }

    fn with_uid(files: &[(&str, &str)], uid: Box<dyn Fn() -> u32 + Send + Sync>) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("licoman-write-{}-{unique}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, content) in files {
            let full = dir.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, content).unwrap();
        }
        let read =
            FileReadBackend::for_dirs(dir.join("config"), dir.join("home"), vec![dir.join("etc")]);
        let write = FileWriteBackend::with_uid(read, dir.join("backups"), uid);
        Self { dir, write }
    }

    fn read(&self) -> FileReadBackend {
        self.write.read_backend().clone()
    }
}

fn make_intent(
    setting_id: &str,
    before: Option<SettingValue>,
    value: SettingValue,
) -> ChangeIntent {
    ChangeIntent {
        setting_id: setting_id.into(),
        before,
        change: Change::Set(value),
        expected_fingerprint: None,
    }
}

fn fingerprint_of(
    fixture: &FileFixture,
    binding: &crate::modules::def::BackendBinding,
    id: &str,
) -> String {
    fixture
        .read()
        .probe_key(binding, &descriptor(id, crate::value::SettingType::Float))
        .unwrap()
        .fingerprint
}

fn apply_file(
    fixture: &FileFixture,
    _setting_id: &str,
    binding: &crate::modules::def::BackendBinding,
    descriptor: &SettingDescriptor,
    intent: &ChangeIntent,
) -> Result<crate::write::ApplyOutcome, crate::error::LicomanError> {
    let request = ChangeRequest::resolve(descriptor, binding, intent, RiskLevel::Low);
    fixture.write.apply(&request, descriptor)
}

// ---- 1. External edit between read and write: conflict, no overwrite --------

#[test]
fn external_edit_after_read_conflicts_and_never_overwrites() {
    let original = "# my config\n[font]\nsize = 11.5\n";
    let fx = FileFixture::new(&[("config/alacritty/alacritty.toml", original)]);
    let binding = file_binding(
        "alacritty/alacritty.toml",
        "font.size",
        Some(FileResolution::Alacritty),
    );
    let descriptor = descriptor("t1", crate::value::SettingType::Float);
    let before = SettingValue::Float(11.5);
    let fingerprint = fingerprint_of(&fx, &binding, "t1");

    // The user edits the file after Licoman read it.
    std::fs::write(
        fx.dir.join("config/alacritty/alacritty.toml"),
        "# edited\n[font]\nsize = 20.0\n",
    )
    .unwrap();

    let mut intent = make_intent("t1", Some(before.clone()), SettingValue::Float(13.0));
    intent.expected_fingerprint = Some(fingerprint);
    let error = apply_file(&fx, "t1", &binding, &descriptor, &intent).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Conflict);
    assert!(error.user_message.contains("changed outside Licoman"));
    // The external edit survives byte for byte.
    assert_eq!(
        std::fs::read_to_string(fx.dir.join("config/alacritty/alacritty.toml")).unwrap(),
        "# edited\n[font]\nsize = 20.0\n"
    );

    // The semantic check catches it even without a fingerprint.
    let mut semantic = make_intent("t1", Some(before), SettingValue::Float(13.0));
    semantic.expected_fingerprint = None;
    let error = apply_file(&fx, "t1", &binding, &descriptor, &semantic).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Conflict);
    assert_eq!(
        std::fs::read_to_string(fx.dir.join("config/alacritty/alacritty.toml")).unwrap(),
        "# edited\n[font]\nsize = 20.0\n"
    );
}

// ---- 2. Comments, custom ordering, unrelated keys: structurally preserved ---

#[test]
fn targeted_patch_preserves_comments_ordering_and_unrelated_keys() {
    let original = "# top comment\n\n[terminal]\nshell = \"/bin/fish\" # user comment\n\n# section note\n[font]\nsize = 11.5\nbold = true\n\n[window]\nopacity = 0.9\n";
    let fx = FileFixture::new(&[("config/alacritty/alacritty.toml", original)]);
    let binding = file_binding(
        "alacritty/alacritty.toml",
        "font.size",
        Some(FileResolution::Alacritty),
    );
    let descriptor = descriptor("t2", crate::value::SettingType::Float);
    let outcome = apply_file(
        &fx,
        "t2",
        &binding,
        &descriptor,
        &make_intent(
            "t2",
            Some(SettingValue::Float(11.5)),
            SettingValue::Float(14.0),
        ),
    )
    .unwrap();
    assert!(outcome.verified);
    let after = std::fs::read_to_string(fx.dir.join("config/alacritty/alacritty.toml")).unwrap();
    assert_eq!(after, original.replace("size = 11.5", "size = 14.0"));
    // Everything unrelated is byte-identical.
    assert!(after.starts_with("# top comment"));
    assert!(after.contains("shell = \"/bin/fish\" # user comment"));
    assert!(after.contains("# section note"));
    assert!(after.contains("bold = true"));
    assert!(after.contains("opacity = 0.9"));
}

// ---- 3. Nested imports: correct target or explicit read-only denial ---------

#[test]
fn imported_winner_is_refused_and_root_winner_edits_the_root() {
    let fx = FileFixture::new(&[
        (
            "config/alacritty/alacritty.toml",
            "font.size = 12.0\n\n[general]\nimport = [\"theme.toml\"]\n",
        ),
        (
            "config/alacritty/theme.toml",
            "font.normal.family = \"Mono\"\nfont.size = 9.0\n",
        ),
    ]);
    let descriptor = descriptor("t3", crate::value::SettingType::Float);

    // The imported file wins font.normal.family → explicit read-only denial.
    let family = file_binding(
        "alacritty/alacritty.toml",
        "font.normal.family",
        Some(FileResolution::Alacritty),
    );
    let error = apply_file(
        &fx,
        "t3",
        &family,
        &descriptor,
        &make_intent(
            "t3",
            Some(SettingValue::Text("Mono".into())),
            SettingValue::Text("Serif".into()),
        ),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Permission);
    assert!(error.user_message.contains("imported"));

    // The root wins font.size → the root file is the edit target.
    let size = file_binding(
        "alacritty/alacritty.toml",
        "font.size",
        Some(FileResolution::Alacritty),
    );
    let outcome = apply_file(
        &fx,
        "t3",
        &size,
        &descriptor,
        &make_intent(
            "t3",
            Some(SettingValue::Float(12.0)),
            SettingValue::Float(13.5),
        ),
    )
    .unwrap();
    assert!(outcome.verified);
    assert_eq!(
        std::fs::read_to_string(fx.dir.join("config/alacritty/alacritty.toml")).unwrap(),
        "font.size = 13.5\n\n[general]\nimport = [\"theme.toml\"]\n"
    );
    // The imported file is untouched.
    assert_eq!(
        std::fs::read_to_string(fx.dir.join("config/alacritty/theme.toml")).unwrap(),
        "font.normal.family = \"Mono\"\nfont.size = 9.0\n"
    );
}

// ---- 4+5. Symlink escape and path traversal are rejected before content ----

#[test]
fn symlink_escape_and_symlink_target_are_rejected() {
    let fx = FileFixture::new(&[
        ("config/alacritty/alacritty.toml", "font.size = 11.0\n"),
        ("outside/secret.toml", "font.size = 1.0\n"),
    ]);
    let descriptor = descriptor("t4", crate::value::SettingType::Float);

    // An import symlink pointing outside the roots is never followed.
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        fx.dir.join("outside/secret.toml"),
        fx.dir.join("config/alacritty/evil.toml"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        let probe = fx
            .read()
            .probe_key(
                &file_binding(
                    "alacritty/alacritty.toml",
                    "font.size",
                    Some(FileResolution::Alacritty),
                ),
                &descriptor,
            )
            .unwrap();
        assert_eq!(probe.root.file_name().unwrap(), "alacritty.toml");
    }

    // A symlink AT the root candidate path is refused for writes (renaming
    // over it would silently replace the user's link with a regular file).
    #[cfg(unix)]
    {
        let link_dir = fx.dir.join("config/link");
        std::fs::create_dir_all(&link_dir).unwrap();
        // (a) in-root link: the write lands on the canonical target inside
        // the approved roots and the user's link is preserved (never
        // replaced by a regular file). Escapes are refused in (b).
        std::os::unix::fs::symlink(
            fx.dir.join("config/alacritty/alacritty.toml"),
            link_dir.join("inside.toml"),
        )
        .unwrap();
        let inside = crate::modules::def::BackendBinding::File {
            path: "link/inside.toml".into(),
            format: FileFormat::Toml,
            key: Some("font.size".into()),
            resolution: Some(FileResolution::XdgFile),
        };
        let outcome = apply_file(
            &fx,
            "t4",
            &inside,
            &descriptor,
            &make_intent(
                "t4",
                Some(SettingValue::Float(11.0)),
                SettingValue::Float(2.0),
            ),
        )
        .unwrap();
        assert!(outcome.verified);
        // The link still exists and still resolves through the roots.
        let link_type = std::fs::symlink_metadata(link_dir.join("inside.toml"))
            .unwrap()
            .file_type();
        assert!(link_type.is_symlink(), "the user's link must be preserved");
        // And the write is visible through it.
        let probe = fx.read().probe_key(&inside, &descriptor).unwrap();
        assert_eq!(probe.value, Some(SettingValue::Float(2.0)));

        // (b) out-of-root link: containment rejects it before any write.
        std::os::unix::fs::symlink(
            fx.dir.join("outside/secret.toml"),
            link_dir.join("linked.toml"),
        )
        .unwrap();
        let outside = crate::modules::def::BackendBinding::File {
            path: "link/linked.toml".into(),
            format: FileFormat::Toml,
            key: Some("font.size".into()),
            resolution: Some(FileResolution::XdgFile),
        };
        let result = apply_file(
            &fx,
            "t4",
            &outside,
            &descriptor,
            &make_intent(
                "t4",
                Some(SettingValue::Float(1.0)),
                SettingValue::Float(2.0),
            ),
        );
        assert!(result.is_err(), "out-of-root link must never be written");
        // The linked file outside the root is untouched.
        assert_eq!(
            std::fs::read_to_string(fx.dir.join("outside/secret.toml")).unwrap(),
            "font.size = 1.0\n"
        );
    }
}

#[test]
fn path_traversal_from_the_binding_is_rejected_before_any_file_access() {
    let fx = FileFixture::new(&[("config/alacritty/alacritty.toml", "font.size = 11.0\n")]);
    let descriptor = descriptor("t5", crate::value::SettingType::Float);
    for evil in ["../../etc/passwd", "/etc/passwd", "a/../../b.toml"] {
        let binding = crate::modules::def::BackendBinding::File {
            path: evil.into(),
            format: FileFormat::Toml,
            key: Some("font.size".into()),
            resolution: Some(FileResolution::XdgFile),
        };
        let error = apply_file(
            &fx,
            "t5",
            &binding,
            &descriptor,
            &make_intent(
                "t5",
                Some(SettingValue::Float(1.0)),
                SettingValue::Float(2.0),
            ),
        )
        .unwrap_err();
        // Containment rejects it; the error is not a successful write.
        assert!(matches!(
            error.kind,
            ErrorKind::Unavailable | ErrorKind::Permission | ErrorKind::Conflict
        ));
    }
    // The fixture root is untouched.
    assert_eq!(
        std::fs::read_to_string(fx.dir.join("config/alacritty/alacritty.toml")).unwrap(),
        "font.size = 11.0\n"
    );
}

// ---- 6. Foreign ownership is rejected ----------------------------------------

#[test]
fn foreign_owned_file_is_rejected_with_permission_error() {
    let fx = FileFixture::with_uid(
        &[("config/alacritty/alacritty.toml", "font.size = 11.0\n")],
        Box::new(|| 12345), // a uid that owns nothing here
    );
    let binding = file_binding(
        "alacritty/alacritty.toml",
        "font.size",
        Some(FileResolution::Alacritty),
    );
    let descriptor = descriptor("t6", crate::value::SettingType::Float);
    let error = apply_file(
        &fx,
        "t6",
        &binding,
        &descriptor,
        &make_intent(
            "t6",
            Some(SettingValue::Float(11.0)),
            SettingValue::Float(12.0),
        ),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Permission);
    assert!(error.user_message.contains("another user"));
    assert_eq!(
        std::fs::read_to_string(fx.dir.join("config/alacritty/alacritty.toml")).unwrap(),
        "font.size = 11.0\n"
    );
}

// ---- 7+9. Disk full during staging; crash before rename ----------------------

#[test]
fn disk_full_during_staging_leaves_the_original_intact() {
    let fx = FileFixture::new(&[("config/alacritty/alacritty.toml", "font.size = 11.0\n")]);
    let binding = file_binding(
        "alacritty/alacritty.toml",
        "font.size",
        Some(FileResolution::Alacritty),
    );
    let descriptor = descriptor("t8", crate::value::SettingType::Float);
    *fx.write.faults().lock().unwrap() = FaultSet {
        enospc_on_stage: true,
        ..FaultSet::default()
    };
    let error = apply_file(
        &fx,
        "t8",
        &binding,
        &descriptor,
        &make_intent(
            "t8",
            Some(SettingValue::Float(11.0)),
            SettingValue::Float(12.0),
        ),
    )
    .unwrap_err();
    assert!(error.user_message.contains("disk is full"));
    // Original intact, staging debris cleaned up.
    assert_eq!(
        std::fs::read_to_string(fx.dir.join("config/alacritty/alacritty.toml")).unwrap(),
        "font.size = 11.0\n"
    );
    let debris: Vec<_> = std::fs::read_dir(fx.dir.join("config/alacritty"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".licoman-"))
        .collect();
    assert!(debris.is_empty(), "staging debris left behind: {debris:?}");
}

#[test]
fn crash_before_rename_is_recovered_by_the_sweep() {
    let fx = FileFixture::new(&[("config/alacritty/alacritty.toml", "font.size = 11.0\n")]);
    let alacritty_dir = fx.dir.join("config/alacritty");
    // Simulate a process that died between temp-create and rename.
    let stale = alacritty_dir.join(".alacritty.toml.licoman-999-1.tmp");
    std::fs::write(&stale, "garbage").unwrap();
    // A fresh backend over the same roots sweeps the stale temp on
    // construction (restart cleanup).
    let read = FileReadBackend::for_dirs(
        fx.dir.join("config"),
        fx.dir.join("home"),
        vec![fx.dir.join("etc")],
    );
    let _sweeper = FileWriteBackend::new(read, fx.dir.join("backups2"));
    assert!(!stale.exists(), "stale temp survived the sweep");
    assert_eq!(
        std::fs::read_to_string(alacritty_dir.join("alacritty.toml")).unwrap(),
        "font.size = 11.0\n"
    );
}

// ---- 8. Readback mismatch → restore; restore failure reported honestly -------

#[test]
fn verification_failure_restores_the_backup() {
    let fx = FileFixture::new(&[("config/alacritty/alacritty.toml", "font.size = 11.0\n")]);
    let binding = file_binding(
        "alacritty/alacritty.toml",
        "font.size",
        Some(FileResolution::Alacritty),
    );
    let descriptor = descriptor("t9", crate::value::SettingType::Float);
    *fx.write.faults().lock().unwrap() = FaultSet {
        readback_mismatch: true,
        ..FaultSet::default()
    };
    let error = apply_file(
        &fx,
        "t9",
        &binding,
        &descriptor,
        &make_intent(
            "t9",
            Some(SettingValue::Float(11.0)),
            SettingValue::Float(12.0),
        ),
    )
    .unwrap_err();
    assert!(error.user_message.contains("restored your original"));
    assert_eq!(
        std::fs::read_to_string(fx.dir.join("config/alacritty/alacritty.toml")).unwrap(),
        "font.size = 11.0\n"
    );

    // No false rollback success: when the restore also fails, the report
    // says so and points at the backup.
    *fx.write.faults().lock().unwrap() = FaultSet {
        readback_mismatch: true,
        restore_fails: true,
        ..FaultSet::default()
    };
    let error = apply_file(
        &fx,
        "t9",
        &binding,
        &descriptor,
        &make_intent(
            "t9",
            Some(SettingValue::Float(11.0)),
            SettingValue::Float(12.0),
        ),
    )
    .unwrap_err();
    assert!(error.user_message.contains("automatic restore failed"));
    assert!(error.technical.contains("backup at"));
}

// ---- Backup retention and privacy ---------------------------------------------

#[test]
fn backups_are_bounded_private_and_redacted_in_the_log() {
    let fx = FileFixture::new(&[("config/alacritty/alacritty.toml", "font.size = 11.0\n")]);
    let binding = file_binding(
        "alacritty/alacritty.toml",
        "font.size",
        Some(FileResolution::Alacritty),
    );
    let descriptor = descriptor("t10", crate::value::SettingType::Float);
    use std::os::unix::fs::PermissionsExt;
    let mut current = Some(SettingValue::Float(11.0));
    for round in 0..15u32 {
        let value = SettingValue::Float(11.0 + f64::from(round + 1));
        apply_file(
            &fx,
            "t10",
            &binding,
            &descriptor,
            &make_intent("t10", current.clone(), value.clone()),
        )
        .unwrap();
        current = Some(value);
    }
    let backups: Vec<_> = std::fs::read_dir(fx.dir.join("backups/t10"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(
        backups.len(),
        crate::write::file_write::MAX_BACKUPS_PER_TARGET
    );
    for entry in &backups {
        let mode = entry.metadata().unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "backup file mode {mode:o} is not private"
        );
    }
    // The engine's operation log records a redacted target — never the
    // absolute temp path, never file contents.
    let engine = test_engine(&fx);
    let secret = "token_sk_9f8e7d6c";
    let request = ChangeRequest::resolve(
        &descriptor,
        &binding,
        &make_intent("t10", None, SettingValue::Text(secret.to_string())),
        RiskLevel::Low,
    );
    // (the value is a string here only to prove the log never carries it)
    let _ = engine.apply(&request, &descriptor);
    for record in engine.operations() {
        assert!(!format!("{record:?}").contains(secret));
        assert!(!record.target.contains(fx.dir.to_string_lossy().as_ref()));
    }
}

fn test_engine(fx: &FileFixture) -> WriteEngine {
    let service = ServiceWriteBackend::new(Arc::new(FakeServiceClient::new()));
    WriteEngine::with_broker(
        FileWriteBackend::with_uid(
            fx.read(),
            fx.dir.join("backups-engine"),
            Box::new(crate::write::file_write::current_uid),
        ),
        service,
        Arc::new(DenyAllBroker),
    )
}

// ---- 10+11. Fake D-Bus service: timeouts, denials, post-apply changes --------

fn service_endpoint() -> (ServiceEndpoint, FakeServiceClient) {
    let client = FakeServiceClient::new();
    let endpoint = ServiceEndpoint {
        bus: ServiceBus::System,
        service: "org.freedesktop.UPower.PowerProfiles".into(),
        path: "/org/freedesktop/UPower/PowerProfiles".into(),
        interface: "org.freedesktop.UPower.PowerProfiles".into(),
    };
    client.seed(
        endpoint.clone(),
        "ActiveProfile",
        PropertyValue::Str("balanced".into()),
        Persistence::RuntimeOnly,
    );
    (endpoint, client)
}

fn service_binding() -> crate::modules::def::BackendBinding {
    crate::modules::def::BackendBinding::DBus {
        service: "org.freedesktop.UPower.PowerProfiles".into(),
        path: "/org/freedesktop/UPower/PowerProfiles".into(),
        interface: "org.freedesktop.UPower.PowerProfiles".into(),
        property: Some("ActiveProfile".into()),
    }
}

#[test]
fn service_timeout_and_denial_are_not_reported_as_applied() {
    let (_endpoint, mut client) = service_endpoint();
    client.set_faults(crate::write::service_write::FakeFaults {
        hang_set: true,
        ..Default::default()
    });
    client.hang_duration = std::time::Duration::from_secs(2);
    let backend =
        ServiceWriteBackend::with_timeout(Arc::new(client), std::time::Duration::from_millis(150));
    let descriptor = descriptor("t11", crate::value::SettingType::Text);
    let request = ChangeRequest::resolve(
        &descriptor,
        &service_binding(),
        &make_intent(
            "t11",
            Some(SettingValue::Choice("balanced".into())),
            SettingValue::Choice("performance".into()),
        ),
        RiskLevel::Low,
    );
    let error = backend.apply(&request, &descriptor).unwrap_err();
    // Honest timeout: the service may have applied it; no false success.
    assert!(error.user_message.contains("did not answer in time"));
    assert!(error.user_message.contains("refresh"));

    // Denial is a permission refusal with nothing modified.
    let (endpoint, client) = service_endpoint();
    client.set_faults(crate::write::service_write::FakeFaults {
        deny_set: true,
        ..Default::default()
    });
    let backend = ServiceWriteBackend::new(Arc::new(client));
    let request = ChangeRequest::resolve(
        &descriptor,
        &service_binding(),
        &make_intent(
            "t11",
            Some(SettingValue::Choice("balanced".into())),
            SettingValue::Choice("performance".into()),
        ),
        RiskLevel::Low,
    );
    let _ = &endpoint;
    let error = backend.apply(&request, &descriptor).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Permission);
}

#[test]
fn service_change_after_apply_is_reported_as_conflict_not_success() {
    use crate::write::service_write::ServiceWriteClient as _;
    let (endpoint, client) = service_endpoint();
    client.set_faults(crate::write::service_write::FakeFaults {
        change_after_apply: Some(PropertyValue::Str("power-saver".into())),
        ..Default::default()
    });
    let client = Arc::new(client);
    let backend = ServiceWriteBackend::new(client.clone());
    let descriptor = descriptor("t12", crate::value::SettingType::Text);
    let request = ChangeRequest::resolve(
        &descriptor,
        &service_binding(),
        &make_intent(
            "t12",
            Some(SettingValue::Choice("balanced".into())),
            SettingValue::Choice("performance".into()),
        ),
        RiskLevel::Low,
    );
    let outcome = backend.apply(&request, &descriptor).unwrap();
    assert!(
        !outcome.verified,
        "a mismatched readback must never claim verified"
    );
    assert_eq!(
        outcome.readback_mismatch,
        Some(SettingValue::Choice("power-saver".into()))
    );
    // The service (authoritative) holds the external value; a fresh read
    // reflects it.
    let current = client.get(&endpoint, "ActiveProfile").unwrap();
    assert_eq!(current, PropertyValue::Str("power-saver".into()));
    // Persistence is honest: runtime only, and the UI says so.
    assert_eq!(outcome.persistence, Persistence::RuntimeOnly);
    let _ = endpoint;
}

#[test]
fn service_conflicts_when_the_current_value_differs_from_the_expectation() {
    let (endpoint, client) = service_endpoint();
    let backend = ServiceWriteBackend::new(Arc::new(client));
    let descriptor = descriptor("t13", crate::value::SettingType::Text);
    let request = ChangeRequest::resolve(
        &descriptor,
        &service_binding(),
        &make_intent(
            "t13",
            Some(SettingValue::Choice("stale".into())),
            SettingValue::Choice("performance".into()),
        ),
        RiskLevel::Low,
    );
    let error = backend.apply(&request, &descriptor).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Conflict);
    let _ = endpoint;
}

// ---- 12. Two backend edits, second fails: partial success, compensation ------

#[test]
fn two_service_edits_with_second_failure_stay_partial_and_undoable() {
    use crate::mock::MockProvider;
    use crate::registry::Registry;

    // A store over two mock-backed staged settings where the second write
    // always fails — the store-level shape of the two-backend batch.
    let ok = crate::mock::MockSettingFixture::new(
        SettingDescriptor::new(
            "a.one",
            Scope::Shared,
            "C",
            "One",
            "",
            crate::value::SettingType::Bool,
            Control::Switch,
        ),
        SettingValue::Bool(false),
    );
    let bad = crate::mock::MockSettingFixture::new(
        SettingDescriptor::new(
            "a.two",
            Scope::Shared,
            "C",
            "Two",
            "",
            crate::value::SettingType::Bool,
            Control::Switch,
        ),
        SettingValue::Bool(false),
    )
    .fail_writes();
    let mut registry = Registry::new();
    registry.register(Box::new(MockProvider::new("pair", vec![ok, bad])));
    let mut store = crate::model::Store::new(
        registry,
        crate::prefs::LoadedPreferences {
            preferences: crate::prefs::Preferences::default(),
            diagnostics: Vec::new(),
        },
        std::env::temp_dir().join("licoman-pair-test-prefs.toml"),
    );
    store.dispatch(crate::command::Command::Edit {
        id: "a.one".into(),
        value: SettingValue::Bool(true),
    });
    store.dispatch(crate::command::Command::Edit {
        id: "a.two".into(),
        value: SettingValue::Bool(true),
    });
    store.apply_begin();
    store.apply_complete();

    // Explicit partial success: the first change is Applied and undoable
    // (its compensation — the recorded before-value — exists), the second
    // is Failed. No global all-or-nothing claim anywhere.
    assert_eq!(store.model("a.one").unwrap().phase, SettingPhase::Applied);
    assert!(store.undo_available("a.one"));
    assert_eq!(store.model("a.two").unwrap().phase, SettingPhase::Failed);

    // The compensation executes only on explicit user action, and succeeds
    // honestly: the source holds the recorded before-value again.
    store.dispatch(crate::command::Command::Undo { id: "a.one".into() });
    store.apply_complete();
    let model = store.model("a.one").unwrap();
    assert_eq!(model.phase, SettingPhase::Applied);
    assert_eq!(model.stored, Some(SettingValue::Bool(false)));
}

// ---- 14. Privileged writes are denied by default -------------------------------

#[test]
fn privileged_request_is_denied_by_the_deny_all_broker() {
    let fx = FileFixture::new(&[("config/alacritty/alacritty.toml", "font.size = 11.0\n")]);
    let mut descriptor = descriptor("t14", crate::value::SettingType::Float);
    descriptor.permission_scope = Some("org.example.SystemConfig".into());
    let binding = file_binding(
        "alacritty/alacritty.toml",
        "font.size",
        Some(FileResolution::Alacritty),
    );
    let request = ChangeRequest::resolve(
        &descriptor,
        &binding,
        &make_intent(
            "t14",
            Some(SettingValue::Float(11.0)),
            SettingValue::Float(12.0),
        ),
        RiskLevel::Low,
    );
    // Core-derived policy: privileged regardless of what anything declared.
    assert_eq!(request.policy, WritePolicy::Privileged);
    let engine = test_engine(&fx);
    let error = engine.apply(&request, &descriptor).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Permission);
    assert!(error.user_message.contains("unprivileged"));
    // Nothing was written.
    assert_eq!(
        std::fs::read_to_string(fx.dir.join("config/alacritty/alacritty.toml")).unwrap(),
        "font.size = 11.0\n"
    );
    assert!(
        engine
            .operations()
            .iter()
            .any(|record| record.status == crate::write::OpStatus::Refused)
    );
}

// ---- 15. Manifest values are data, never commands ------------------------------

#[test]
fn shell_text_in_values_is_stored_verbatim_and_never_executed() {
    let fx = FileFixture::new(&[("config/alacritty/alacritty.toml", "font.size = 11.0\n")]);
    let binding = file_binding(
        "alacritty/alacritty.toml",
        "font.size",
        Some(FileResolution::Alacritty),
    );
    let engine = test_engine(&fx);
    let float_descriptor = descriptor("t15", crate::value::SettingType::Float);
    let hostile = "11.0; import os; os.system('rm -rf /')";
    let outcome = engine.apply(
        &ChangeRequest::resolve(
            &float_descriptor,
            &binding,
            &make_intent(
                "t15",
                Some(SettingValue::Float(11.0)),
                SettingValue::Text(hostile.to_string()),
            ),
            RiskLevel::Low,
        ),
        &float_descriptor,
    );
    // A float key cannot take text: the mismatch is a validation failure
    // before any I/O (the value gate), and nothing is written.
    assert_eq!(outcome.unwrap_err().kind, ErrorKind::Validation);
    assert_eq!(
        std::fs::read_to_string(fx.dir.join("config/alacritty/alacritty.toml")).unwrap(),
        "font.size = 11.0\n"
    );

    // A string key stores the text verbatim — data, never executed.
    let fx = FileFixture::new(&[("config/app/config.toml", "shell = \"/bin/sh\"\n")]);
    let engine = test_engine(&fx);
    let binding = crate::modules::def::BackendBinding::File {
        path: "app/config.toml".into(),
        format: FileFormat::Toml,
        key: Some("shell".into()),
        resolution: Some(FileResolution::XdgFile),
    };
    let text_descriptor = descriptor("t15b", crate::value::SettingType::Text);
    engine
        .apply(
            &ChangeRequest::resolve(
                &text_descriptor,
                &binding,
                &make_intent(
                    "t15b",
                    Some(SettingValue::Text("/bin/sh".into())),
                    SettingValue::Text(hostile.to_string()),
                ),
                RiskLevel::Low,
            ),
            &text_descriptor,
        )
        .unwrap();
    let after = std::fs::read_to_string(fx.dir.join("config/app/config.toml")).unwrap();
    assert!(after.contains(hostile), "value must land verbatim as data");
}

// ---- GSettings engine gate + mock-target guard at engine level ----------------
//
// Phase 07 semantics (07_FIRST_REAL_INTEGRATIONS.md Integration B): an
// engine without a GSettings backend refuses GSettings writes (honest gate);
// an engine **with** one additionally requires the fail-closed desktop gate
// (covered by the gsettings_write suite, including the memory-backend round
// trip). Nothing here or in CI touches a real dconf database.

#[test]
fn gsettings_writes_refuse_without_a_backend_and_mock_targets_stay_refused() {
    let fx = FileFixture::new(&[]);
    let engine = test_engine(&fx);
    let mut descriptor = descriptor("t16", crate::value::SettingType::Bool);
    descriptor.control = Control::Switch;
    let gsettings = crate::modules::def::BackendBinding::GSettings {
        schema: "org.gnome.desktop.interface".into(),
        key: "enable-animations".into(),
    };
    let request = ChangeRequest::resolve(
        &descriptor,
        &gsettings,
        &make_intent(
            "t16",
            Some(SettingValue::Bool(true)),
            SettingValue::Bool(false),
        ),
        RiskLevel::Low,
    );
    let error = engine.apply(&request, &descriptor).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Permission);
    assert!(
        error
            .user_message
            .contains("GSettings writes are not enabled")
    );

    let request = ChangeRequest::resolve(
        &descriptor,
        &crate::modules::def::BackendBinding::Mock,
        &make_intent(
            "t16",
            Some(SettingValue::Bool(true)),
            SettingValue::Bool(false),
        ),
        RiskLevel::Low,
    );
    let error = engine.apply(&request, &descriptor).unwrap_err();
    assert!(error.technical.contains("mock"));
}

// ---- Reset semantics -------------------------------------------------------------

#[test]
fn remove_override_deletes_only_the_target_key_and_preserves_the_rest() {
    let original = "[font]\nsize = 11.5\nbold = true\n\n[window]\nopacity = 0.9\n";
    let fx = FileFixture::new(&[("config/alacritty/alacritty.toml", original)]);
    let binding = file_binding(
        "alacritty/alacritty.toml",
        "font.size",
        Some(FileResolution::Alacritty),
    );
    let descriptor = descriptor("t17", crate::value::SettingType::Float);
    let request = ChangeRequest::resolve(
        &descriptor,
        &binding,
        &ChangeIntent {
            setting_id: "t17".into(),
            before: Some(SettingValue::Float(11.5)),
            change: Change::RemoveOverride,
            expected_fingerprint: None,
        },
        RiskLevel::Low,
    );
    let outcome = fx.write.apply(&request, &descriptor).unwrap();
    assert!(outcome.verified);
    assert!(outcome.expect_absent);
    let after = std::fs::read_to_string(fx.dir.join("config/alacritty/alacritty.toml")).unwrap();
    assert!(!after.contains("size"));
    assert!(after.contains("bold = true"));
    assert!(after.contains("opacity = 0.9"));

    // Removing again is an honest verified no-op: the expectation now is
    // "key absent" (before = None), matching the file.
    let again = ChangeRequest::resolve(
        &descriptor,
        &binding,
        &ChangeIntent {
            setting_id: "t17".into(),
            before: None,
            change: Change::RemoveOverride,
            expected_fingerprint: None,
        },
        RiskLevel::Low,
    );
    let outcome = fx.write.apply(&again, &descriptor).unwrap();
    assert!(outcome.verified);
}

// ---- Insertion creates dotted keys without disturbing other content --------------

#[test]
fn setting_an_absent_key_creates_it_without_touching_other_sections() {
    let original = "[window]\nopacity = 0.9\n";
    let fx = FileFixture::new(&[("config/alacritty/alacritty.toml", original)]);
    let binding = file_binding(
        "alacritty/alacritty.toml",
        "font.size",
        Some(FileResolution::Alacritty),
    );
    let descriptor = descriptor("t18", crate::value::SettingType::Float);
    let outcome = apply_file(
        &fx,
        "t18",
        &binding,
        &descriptor,
        &make_intent("t18", None, SettingValue::Float(12.5)),
    )
    .unwrap();
    assert!(outcome.verified);
    let after = std::fs::read_to_string(fx.dir.join("config/alacritty/alacritty.toml")).unwrap();
    // The insertion is minimal: one new section for the new key; the
    // existing table is untouched.
    assert!(after.contains("[font]"));
    assert!(after.contains("size = 12.5"));
    assert!(after.contains("[window]"));
    assert!(after.contains("opacity = 0.9"));
    // The result parses and resolves through the read side.
    let probe = fx.read().probe_key(&binding, &descriptor).unwrap();
    assert_eq!(probe.value, Some(SettingValue::Float(12.5)));
}

// ---- Full-stack: store → registry → provider → engine → file -----------------

#[test]
fn full_stack_store_to_file_write_conflict_undo_and_reset() {
    let config = "font.size = 11.0\n\n[font.normal]\nfamily = \"Mono\"\n";
    let fx = FileFixture::new(&[("config/app/config.toml", config)]);
    let binding_text: String = String::from(
        r#"
schema_version = 1
id = "app.stack"
scope = "application"
title = "Stack"
category = "Test"

[[settings]]
id = "size"
title = "Size"
type = "float"
control = "slider"
min = 1
max = 40
step = 0.5
default = 11

[settings.backend]
kind = "file"
path = "app/config.toml"
format = "toml"
key = "font.size"
resolution = "xdg_file"
"#,
    );
    let module_registry = Arc::new(std::sync::Mutex::new(crate::modules::ModuleRegistry::load(
        [(crate::modules::ModuleSource::Builtin("stack"), binding_text)],
    )));
    let read = fx.read();
    let engine = WriteEngine::new(
        FileWriteBackend::with_uid(
            read.clone(),
            fx.dir.join("backups"),
            Box::new(crate::write::file_write::current_uid),
        ),
        ServiceWriteBackend::new(Arc::new(FakeServiceClient::new())),
    );
    let provider = crate::modules::ModuleProvider::with_fixture_write_engine(
        module_registry.clone(),
        crate::read::BackendRouter::new(
            crate::read::gsettings::GSettingsReadBackend::for_paths(vec![], None, vec![]),
            read,
            crate::read::dbus::DBusReadBackend::with_client(Arc::new(
                crate::read::dbus::AbsentServiceClient,
            )),
        ),
        engine,
    );
    provider.set_environment(Some(
        &crate::discovery::demo_fixtures::arch_hyprland_wayland(),
    ));

    let mut core_registry = crate::registry::Registry::new();
    core_registry.register(Box::new(crate::modules::ModuleProvider::clone(&provider)));
    let mut store = crate::model::Store::new(
        core_registry,
        crate::prefs::LoadedPreferences {
            preferences: crate::prefs::Preferences::default(),
            diagnostics: Vec::new(),
        },
        std::env::temp_dir().join("licoman-stack-test-prefs.toml"),
    );
    let id = "app.stack.size";

    // Simulate the adapter's background read pass (the row is
    // Background-latency and starts Unknown).
    let read_result = crate::provider::Provider::read_detailed(&provider, id).unwrap();
    assert!(matches!(read_result.writable, crate::read::Writable::Yes));
    store.publish_read(id, Ok(read_result));
    assert_eq!(store.model(id).unwrap().phase, SettingPhase::Idle);
    let fingerprint = store
        .model(id)
        .unwrap()
        .read_meta
        .as_ref()
        .unwrap()
        .freshness
        .fingerprint
        .clone();

    // Stage + apply: the file really changes through the whole stack.
    store.dispatch(crate::command::Command::Edit {
        id: id.into(),
        value: SettingValue::Float(14.0),
    });
    store.apply_begin();
    store.apply_complete();
    let model = store.model(id).unwrap();
    assert_eq!(model.phase, SettingPhase::Applied);
    assert_eq!(model.stored, Some(SettingValue::Float(14.0)));
    assert_eq!(
        std::fs::read_to_string(fx.dir.join("config/app/config.toml")).unwrap(),
        "font.size = 14.0\n\n[font.normal]\nfamily = \"Mono\"\n"
    );
    assert!(store.undo_available(id));

    // Undo restores 11.0 through the same engine path (conflict-checked,
    // backed up, verified).
    store.dispatch(crate::command::Command::Undo { id: id.into() });
    store.apply_begin();
    store.apply_complete();
    let model = store.model(id).unwrap();
    assert_eq!(model.phase, SettingPhase::Applied);
    assert_eq!(model.stored, Some(SettingValue::Float(11.0)));
    assert_eq!(
        std::fs::read_to_string(fx.dir.join("config/app/config.toml")).unwrap(),
        "font.size = 11.0\n\n[font.normal]\nfamily = \"Mono\"\n"
    );

    // Re-apply 14.0 for the conflict scenario.
    store.dispatch(crate::command::Command::Edit {
        id: id.into(),
        value: SettingValue::Float(14.0),
    });
    store.apply_begin();
    store.apply_complete();
    assert_eq!(store.model(id).unwrap().phase, SettingPhase::Applied);

    // A second edit staged BEFORE an external modification (the intent
    // carries the first read's revision) conflicts instead of overwriting
    // once the file changed behind Licoman's back.
    store.dispatch(crate::command::Command::Edit {
        id: id.into(),
        value: SettingValue::Float(16.0),
    });
    assert!(!fingerprint.is_empty(), "sanity: a revision was read");
    std::fs::write(
        fx.dir.join("config/app/config.toml"),
        "font.size = 21.5\n\n[font.normal]\nfamily = \"Mono\"\n",
    )
    .unwrap();
    store.apply_begin();
    store.apply_complete();
    assert_eq!(
        store.model(id).unwrap().phase,
        SettingPhase::ExternalConflict
    );
    // The external edit survives byte for byte — never overwritten.
    assert_eq!(
        std::fs::read_to_string(fx.dir.join("config/app/config.toml")).unwrap(),
        "font.size = 21.5\n\n[font.normal]\nfamily = \"Mono\"\n"
    );

    // Reload accepts the source; after the external change the recorded
    // undo no longer applies (the source no longer holds the applied
    // value), so undo is honestly unavailable.
    store.dispatch(crate::command::Command::Reload { id: id.into() });
    store.dispatch(crate::command::Command::Discard { id: id.into() });
    assert_eq!(store.model(id).unwrap().phase, SettingPhase::Idle);
    assert!(!store.undo_available(id));

    // Reset deletes the override (disclosed), leaving the app default.
    let support = store.reset_support(id).expect("file resets are offered");
    assert!(matches!(
        support.disclosure,
        crate::write::ResetDisclosure::DeletesOverride { .. }
    ));
    store.dispatch(crate::command::Command::ResetToDefault { id: id.into() });
    store.apply_begin();
    store.apply_complete();
    let model = store.model(id).unwrap();
    assert_eq!(model.phase, SettingPhase::Applied);
    assert_eq!(
        std::fs::read_to_string(fx.dir.join("config/app/config.toml")).unwrap(),
        "\n[font.normal]\nfamily = \"Mono\"\n"
    );
}
