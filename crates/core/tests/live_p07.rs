//! Phase 07 controlled live verification (07_FIRST_REAL_INTEGRATIONS.md,
//! "Controlled live verification protocol").
//!
//! **This harness never runs automatically.** It is `#[ignore]`-by-default
//! **and** additionally requires `LICOMAN_LIVE_P07=1` in the environment,
//! so `cargo test` and CI can never reach it. Every run must be explicitly
//! requested, one operation at a time, after user approval of the exact
//! operation (see docs/live-testing/PHASE_07_LIVE_CHECKLIST.md §4).
//!
//! The harness drives the same store → engine → backend path the UI drives
//! (Edit → apply pass → publish_apply), which is how live verification is
//! automated on a machine without an input injector. Native-tool
//! verification reads happen through the documented `gsettings`/`dconf`
//! CLIs — reads only; the writes all go through Licoman's typed engine.
//!
//! Modes (LICOMAN_LIVE_P07_MODE):
//! - `disposable` (default): the process runs with `XDG_CONFIG_HOME` at a
//!   fresh temp directory, so libdconf creates a **throwaway** user
//!   database. The real dconf db is fingerprinted before/after and asserted
//!   byte-identical.
//! - `real`: targets the real user db — only with separate explicit
//!   approval; the pre-change value is recorded and restored via the
//!   platform's real reset semantics.

use std::path::PathBuf;
use std::process::Command as StdCommand;

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use licoman_core::modules::ModuleProvider;
use licoman_core::modules::registry::ModuleSource;
use licoman_core::provider::Provider as _;
use licoman_core::{Command, Registry, SettingPhase, SettingValue, Store};

const MANIFEST: &str = r#"
schema_version = 1
id = "desktop.gnome.interface"
scope = "desktop"
title = "GNOME Interface"
category = "Appearance"

[compatibility]
desktops = ["GNOME"]
required_backends = ["gsettings"]

[[settings]]
id = "clock.show_weekday"
title = "Show weekday in clock"
type = "boolean"
control = "switch"
policy = "immediate"
default = false

[settings.backend]
kind = "gsettings"
schema = "org.gnome.desktop.interface"
key = "clock-show-weekday"
"#;

const SETTING: &str = "desktop.gnome.interface.clock.show_weekday";
const SCHEMA: &str = "org.gnome.desktop.interface";
const KEY: &str = "clock-show-weekday";

/// Build the live store: real router (env-driven), real write engine with
/// the native GSettings backend, gate opened by the fixture GNOME snapshot
/// (the controlled declaration the checklist documents).
fn live_store(config_home: &std::path::Path) -> (Store, Arc<ModuleProvider>, PathBuf) {
    let registry_handle = Arc::new(StdMutex::new(licoman_core::modules::ModuleRegistry::load(
        [(ModuleSource::Builtin("live"), MANIFEST.into())],
    )));
    let provider = Arc::new(ModuleProvider::live(registry_handle.clone()));
    provider.set_environment(Some(
        &licoman_core::discovery::demo_fixtures::fedora_gnome_wayland(),
    ));

    let mut registry = Registry::new();
    registry.register(Box::new(ModuleProvider::clone(&provider)));

    let prefs_dir = std::env::temp_dir().join("licoman-live-p07-prefs");
    let _ = std::fs::create_dir_all(&prefs_dir);
    let prefs_path = prefs_dir.join("preferences.toml");
    let loaded = licoman_core::prefs::load_from(&prefs_path);
    (
        Store::new(registry, loaded, prefs_path),
        provider,
        config_home.to_path_buf(),
    )
}

/// Read a key through the documented `gsettings` CLI with the given config
/// home (verification only — the CLI never writes here).
fn gsettings_get(config_home: &std::path::Path) -> String {
    let output = StdCommand::new("gsettings")
        .arg("get")
        .arg(SCHEMA)
        .arg(KEY)
        .env("XDG_CONFIG_HOME", config_home)
        .output()
        .expect("gsettings CLI available");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Poll the CLI until it agrees with `expected` (or time out). dconf-service
/// flushes the gvdb file asynchronously; a client-side read (CLI or any
/// other process) can trail the in-process engine readback by a moment.
fn gsettings_get_eventually(config_home: &std::path::Path, expected: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut attempts = 0;
    loop {
        let value = gsettings_get(config_home);
        if value == expected || std::time::Instant::now() > deadline {
            println!("CLI verification converged after {attempts} attempts: {value}");
            return value;
        }
        attempts += 1;
        if attempts % 10 == 1 {
            let dump = StdCommand::new("dconf")
                .args(["dump", "/org/gnome/desktop/interface/"])
                .env("XDG_CONFIG_HOME", config_home)
                .output()
                .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
                .unwrap_or_default();
            println!("attempt {attempts}: gsettings get={value:?} dconf dump={dump:?}");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// The whole operation: read → set true → read back → reset → read back.
fn run_operation(config_home: &std::path::Path, label: &str) {
    println!("== live operation ({label}) ==");
    let (mut store, provider, _config_home_owned) = live_store(config_home);

    // Wait for the background read pass equivalent: the store defers
    // Background-latency rows; read synchronously through the provider to
    // establish the pre-state, then publish it into the model the way the
    // adapter does.
    let pre = provider.read_detailed(SETTING).expect("pre-read");
    let pre_value = pre.value.clone();
    println!("pre-state: {:?} (origin {:?})", pre.value, pre.origin);
    assert_eq!(
        pre_value,
        SettingValue::Bool(false),
        "unexpected pre-state; aborting"
    );
    store.publish_read(SETTING, Ok(pre));

    // IMPORTANT (lesson from the first L1 attempt, 2026-11-01): a process
    // environment override does NOT isolate dconf writes — libdconf sends
    // them to the session bus's dconf-service, which resolves its database
    // from its own environment. Disposable isolation therefore requires a
    // PRIVATE session bus (dbus-run-session) whose dconf-service inherits
    // the temp XDG_CONFIG_HOME. The real database's before/after fingerprint
    // is taken by the shell procedure outside this process and recorded in
    // the report; inside here we verify the temp database itself.
    let temp_db = config_home.join("dconf/user");

    // --- set to true through the store's apply path ----------------------
    store.dispatch(Command::Edit {
        id: SETTING.into(),
        value: SettingValue::Bool(true),
    });
    assert!(store.applying(), "immediate edit stages an apply");
    store.apply_complete();
    let model = store.model(SETTING).expect("model exists");
    assert_eq!(model.phase, SettingPhase::Applied, "engine-verified apply");
    assert_eq!(
        model.stored,
        Some(SettingValue::Bool(true)),
        "verification read holds the new value"
    );
    let via_cli_after_set = gsettings_get_eventually(config_home, "true");
    println!(
        "after set: store phase={:?} gsettings CLI={via_cli_after_set}",
        model.phase
    );
    assert_eq!(
        via_cli_after_set, "true",
        "native tool must agree with Licoman"
    );
    assert!(
        temp_db.is_file(),
        "the disposable dconf database must exist and hold the write"
    );

    // --- reset through the platform's real reset semantics ---------------
    let support = store.reset_support(SETTING).expect("reset supported");
    assert!(matches!(
        support.disclosure,
        licoman_core::write::ResetDisclosure::DeletesOverride { .. }
    ));
    store.dispatch(Command::ResetToDefault { id: SETTING.into() });
    store.apply_complete();
    let model = store.model(SETTING).expect("model exists");
    assert_eq!(model.phase, SettingPhase::Applied, "engine-verified reset");
    assert_eq!(
        model.stored,
        Some(SettingValue::Bool(false)),
        "the schema default applies again"
    );
    let via_cli_after_reset = gsettings_get_eventually(config_home, "false");
    println!("after reset: gsettings CLI={via_cli_after_reset}");
    assert_ne!(
        via_cli_after_set, via_cli_after_reset,
        "the override changed something"
    );
    assert_eq!(
        via_cli_after_reset, "false",
        "native tool confirms the override is gone"
    );

    // After the reset the disposable db holds no override for the key.
    if std::env::var("LICOMAN_LIVE_P07_MODE").as_deref() != Ok("real") {
        let dump = StdCommand::new("dconf")
            .args(["dump", "/org/gnome/desktop/interface/"])
            .env("XDG_CONFIG_HOME", config_home)
            .output()
            .expect("dconf CLI available");
        let dump = String::from_utf8_lossy(&dump.stdout);
        assert!(
            !dump.contains(KEY),
            "the disposable db must no longer override the key: {dump}"
        );
        println!("disposable db dump after reset: no {KEY} override ✓");
    }
    println!("== operation complete ({label}) ==");
}

#[test]
#[ignore = "live verification: requires LICOMAN_LIVE_P07=1 and explicit user approval (docs/live-testing/PHASE_07_LIVE_CHECKLIST.md)"]
fn live_gsettings_set_and_reset() {
    if std::env::var("LICOMAN_LIVE_P07").as_deref() != Ok("1") {
        eprintln!("skipped: LICOMAN_LIVE_P07 is not set (this harness never runs by accident)");
        return;
    }
    let mode = std::env::var("LICOMAN_LIVE_P07_MODE").unwrap_or_else(|_| "disposable".into());
    match mode.as_str() {
        "disposable" => {
            // Fresh temp config home: libdconf creates a throwaway user db
            // there. Set before any gio use in this process (a dedicated
            // test binary; no gio use has happened yet).
            let root = std::env::temp_dir().join(format!(
                "licoman-live-p07-disposable-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            let config_home = root.join("config");
            std::fs::create_dir_all(&config_home).expect("temp config home");
            // SAFETY: single-threaded at this point of the dedicated test
            // process, before any library reads the environment.
            unsafe { std::env::set_var("XDG_CONFIG_HOME", &config_home) };
            // Own the session's dconf-service: without this, dbus activation
            // may route through the systemd user instance, whose environment
            // carries the REAL config home (found the hard way — see the
            // report). A pre-started service with our env owns the bus name,
            // so every client in this session — writer and verifiers alike —
            // shares one service with the disposable database.
            // Owns the bus name for this private session; reaped after the
            // operation (its socket dies with the session anyway).
            let mut service = StdCommand::new("/usr/lib/dconf-service")
                .env("XDG_CONFIG_HOME", &config_home)
                .spawn()
                .expect("dconf-service must be startable in the private session");
            std::thread::sleep(std::time::Duration::from_millis(300));
            run_operation(&config_home, "disposable temp dconf db");
            let _ = service.kill();
            let _ = service.wait();
            let _ = std::fs::remove_dir_all(&root);
        }
        "real" => {
            let config_home = std::env::var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    PathBuf::from(std::env::var("HOME").expect("HOME")).join(".config")
                });
            run_operation(&config_home, "REAL user dconf db (approved op)");
        }
        other => panic!("unknown LICOMAN_LIVE_P07_MODE {other:?}"),
    }
}
