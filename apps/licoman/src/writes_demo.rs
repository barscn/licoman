//! The Phase 06 write demos (`LICOMAN_DEMO=writes`, `writes-conflict`,
//! `writes-permission`): a **fixture-backed safe write engine** over a
//! throwaway temp copy of the committed test fixtures plus a fake D-Bus
//! service.
//!
//! Nothing outside the temp directory is ever written: the fixture tree is
//! copied in at startup, the fake service lives in memory, and the whole
//! environment is deleted when the process exits. This is the visual
//! verification harness for the Phase 06 flows — the live run still has no
//! real write path.

use std::path::PathBuf;
use std::sync::Arc;

use licoman_core::modules::ModuleProvider;
use licoman_core::read::BackendRouter;
use licoman_core::read::dbus::{DBusReadBackend, PropertyValue};
use licoman_core::read::file::FileReadBackend;
use licoman_core::read::gsettings::GSettingsReadBackend;
use licoman_core::read::xfconf::{AbsentXfconfClient, XfconfReadBackend};
use licoman_core::write::WriteEngine;
use licoman_core::write::file_write::FileWriteBackend;
use licoman_core::write::service_write::{FakeServiceClient, ServiceWriteBackend};

/// The fixture write environment: temp roots + fake service + engine.
pub struct WritesEnv {
    temp: PathBuf,
    pub dbus: Arc<FakeServiceClient>,
}

impl Drop for WritesEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.temp);
    }
}

impl WritesEnv {
    /// Copy the committed alacritty fixture into a fresh temp tree, seed the
    /// fake power-profiles service, and build the fixture-backed provider.
    pub fn create() -> std::io::Result<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Sweep environments left by instances that were killed hard (their
        // Drop never ran). Bounded, prefixed, temp-only.
        if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("licoman-writes-demo-")
                {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        let temp = std::env::temp_dir().join(format!(
            "licoman-writes-demo-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        for dir in ["config", "home", "etc", "backups"] {
            std::fs::create_dir_all(temp.join(dir))?;
        }
        let fixtures =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/alacritty");
        copy_tree(&fixtures.join("config"), &temp.join("config"))?;
        copy_tree(&fixtures.join("home"), &temp.join("home"))?;
        copy_tree(&fixtures.join("etc"), &temp.join("etc"))?;

        let dbus = Arc::new(FakeServiceClient::new());
        let endpoint = licoman_core::write::ServiceEndpoint {
            bus: licoman_core::write::ServiceBus::System,
            service: "org.freedesktop.UPower.PowerProfiles".into(),
            path: "/org/freedesktop/UPower/PowerProfiles".into(),
            interface: "org.freedesktop.UPower.PowerProfiles".into(),
        };
        dbus.seed(
            endpoint,
            "ActiveProfile",
            PropertyValue::Str("balanced".into()),
            licoman_core::Persistence::RuntimeOnly,
        );
        let profiles_endpoint = licoman_core::write::ServiceEndpoint {
            bus: licoman_core::write::ServiceBus::System,
            service: "org.freedesktop.UPower.PowerProfiles".into(),
            path: "/org/freedesktop/UPower/PowerProfiles".into(),
            interface: "org.freedesktop.UPower.PowerProfiles".into(),
        };
        dbus.seed(
            profiles_endpoint,
            "Profiles",
            PropertyValue::StrList(vec![
                "performance".into(),
                "balanced".into(),
                "power-saver".into(),
            ]),
            licoman_core::Persistence::Persistent,
        );

        Ok(Self { temp, dbus })
    }

    /// The fixture-backed module provider: reads and writes both flow
    /// through the temp roots; the write engine carries the deny-by-default
    /// privilege broker.
    pub fn provider(
        &self,
        registry: Arc<std::sync::Mutex<licoman_core::modules::ModuleRegistry>>,
    ) -> ModuleProvider {
        let read = FileReadBackend::for_dirs(
            self.temp.join("config"),
            self.temp.join("home"),
            vec![self.temp.join("etc")],
        );
        // GSettings reads come from the committed fixture databases, never
        // from the host's real dconf (demo isolation).
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
        let router = BackendRouter::new(
            GSettingsReadBackend::for_paths(
                vec![fixtures.join("discovery/schemas")],
                Some(fixtures.join("gsettings/dconf/user")),
                vec![fixtures.join("gsettings/dconf/system/site")],
            ),
            read.clone(),
            DBusReadBackend::with_client(self.dbus.clone()),
            XfconfReadBackend::with_client(Arc::new(AbsentXfconfClient)),
        );
        let file_write = FileWriteBackend::new(read, self.temp.join("backups"));
        let service_write = ServiceWriteBackend::new(self.dbus.clone());
        let engine = WriteEngine::new(file_write, service_write);
        ModuleProvider::with_fixture_write_engine(registry, router, engine)
    }

    /// The fake service's active profile changes behind Licoman's back (the
    /// conflict demo): a genuine mutation of the demo service state, in
    /// memory only.
    pub fn inject_external_service_change(&self) {
        use licoman_core::write::service_write::ServiceWriteClient as _;
        let endpoint = licoman_core::write::ServiceEndpoint {
            bus: licoman_core::write::ServiceBus::System,
            service: "org.freedesktop.UPower.PowerProfiles".into(),
            path: "/org/freedesktop/UPower/PowerProfiles".into(),
            interface: "org.freedesktop.UPower.PowerProfiles".into(),
        };
        let _ = self.dbus.set(
            &endpoint,
            "ActiveProfile",
            PropertyValue::Str("power-saver".into()),
        );
    }
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    if !from.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
