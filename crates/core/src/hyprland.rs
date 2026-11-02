//! Read-only Hyprland compositor diagnostics (07_FIRST_REAL_INTEGRATIONS.md,
//! "Optional desktop-specific read-only demonstration"): a few confirmed
//! runtime facts from the **documented IPC** (`$XDG_RUNTIME_DIR/hypr/<signature>/
//! .socket.sock`, the socket `hyprctl` itself uses), each clearly labeled
//! runtime-only. Nothing here writes anything — no Hyprland config, no
//! sourced files, no scripts; the plan reserves a safe adapter for user-
//! defined config structures to the later contributor/expansion phase.
//!
//! Requests are written as one bare command (no trailing newline) and the
//! connection is closed for writing; the compositor answers and closes.
//! Facts are read live per request; when the compositor is absent the
//! provider reports itself unavailable and every row shows the honest
//! state. Responses are bounded and parsed defensively — an unparseable
//! reply is an error with a user-safe message, never an invented value.

use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use crate::descriptor::{Control, Scope, SettingDescriptor};
use crate::error::{ErrorKind, LicomanError};
use crate::provider::{ProbeResult, Provider};
use crate::value::{SettingType, SettingValue};

/// How long one IPC round trip may take.
const IPC_TIMEOUT: Duration = Duration::from_secs(2);
/// Read bound for one IPC response.
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// The documented IPC commands this provider uses. Human-readable output is
/// deliberately chosen over the `-j` JSON variants so no JSON parser is
/// pulled into the core for a diagnostics-only feature.
const COMMAND_VERSION: &str = "version";
const COMMAND_ACTIVE_WORKSPACE: &str = "activeworkspace";
const COMMAND_MONITORS: &str = "monitors";

/// The read-only diagnostics provider. Stateless (the socket is resolved
/// per call, so a compositor restart with a new signature keeps working);
/// `Clone` lets the binary register one copy with the store registry and
/// share another with the UI read pass.
#[derive(Clone)]
pub struct HyprlandDiagnostics {
    /// Injectable for tests (a fixture socket). Live: resolved per call
    /// from the environment so a compositor restart (new signature) works.
    socket_override: Option<PathBuf>,
}

impl Default for HyprlandDiagnostics {
    fn default() -> Self {
        Self::new()
    }
}

impl HyprlandDiagnostics {
    pub fn new() -> Self {
        Self {
            socket_override: None,
        }
    }

    /// Fixture constructor pointing at a test socket.
    pub fn with_socket(socket: PathBuf) -> Self {
        Self {
            socket_override: Some(socket),
        }
    }

    /// The documented runtime socket path, when the compositor exposes it.
    fn socket_path(&self) -> Option<PathBuf> {
        // A socket file is not `is_file()` (that is for regular files), so
        // existence is the check.
        if let Some(path) = &self.socket_override {
            return path.exists().then(|| path.clone());
        }
        let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
        let runtime = std::env::var("XDG_RUNTIME_DIR").ok()?;
        let path = PathBuf::from(runtime)
            .join("hypr")
            .join(signature)
            .join(".socket.sock");
        path.exists().then_some(path)
    }

    /// One documented IPC round trip: request line in, whole response out.
    fn request(&self, command: &str) -> Result<String, LicomanError> {
        let path = self.socket_path().ok_or_else(|| {
            LicomanError::new(
                ErrorKind::Unavailable,
                "The Hyprland IPC socket is not available, so compositor \
                 diagnostics cannot be read.",
                "no HYPRLAND_INSTANCE_SIGNATURE/XDG_RUNTIME_DIR socket",
            )
        })?;
        let (tx, rx) = std::sync::mpsc::channel();
        let path_for_thread = path.clone();
        let command = command.to_string();
        std::thread::Builder::new()
            .name("licoman-hypr-ipc".into())
            .spawn(move || {
                let result = (|| -> std::io::Result<String> {
                    let mut stream = UnixStream::connect(&path_for_thread)?;
                    stream.set_read_timeout(Some(IPC_TIMEOUT))?;
                    stream.set_write_timeout(Some(IPC_TIMEOUT))?;
                    // One write, no trailing newline: Hyprland 0.56 answers
                    // the bare command; a newline-terminated query request
                    // is answered "unknown request" on this build (verified
                    // live against 0.56.2 — dispatch commands tolerate the
                    // newline, info commands do not).
                    stream.write_all(command.as_bytes())?;
                    stream.shutdown(std::net::Shutdown::Write)?;
                    let mut response = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        let read = stream.read(&mut chunk)?;
                        if read == 0 {
                            break;
                        }
                        response.extend_from_slice(&chunk[..read]);
                        if response.len() > MAX_RESPONSE_BYTES {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "response exceeds the read bound",
                            ));
                        }
                    }
                    Ok(String::from_utf8_lossy(&response).trim_end().to_string())
                })();
                let _ = tx.send(result);
            })
            .map_err(|error| {
                LicomanError::new(
                    ErrorKind::Unavailable,
                    "The Hyprland diagnostics could not be queried.",
                    format!("worker spawn: {error}"),
                )
            })?;
        match rx.recv_timeout(IPC_TIMEOUT) {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => Err(LicomanError::new(
                ErrorKind::Unavailable,
                "The Hyprland IPC socket did not answer, so compositor \
                 diagnostics cannot be read right now.",
                format!("{}: {error}", path.display()),
            )),
            Err(_) => Err(LicomanError::new(
                ErrorKind::Unavailable,
                "The Hyprland IPC socket did not answer in time. Try Refresh \
                 later.",
                format!("timeout after {IPC_TIMEOUT:?}"),
            )),
        }
    }

    fn read_row(&self, id: &str) -> Result<SettingValue, LicomanError> {
        let value = match id {
            "hyprland.runtime.version" => {
                let response = self.request(COMMAND_VERSION)?;
                // Documented `hyprctl version`-style output; the first line
                // names the build, a `Tag: v…` line carries the release.
                let tag = response
                    .lines()
                    .find_map(|line| line.trim().strip_prefix("Tag:"))
                    .map(|tag| {
                        // "Tag: v0.56.2, commits: 12" → "0.56.2"
                        tag.trim()
                            .split(',')
                            .next()
                            .unwrap_or_default()
                            .trim()
                            .trim_start_matches('v')
                            .to_string()
                    })
                    .filter(|tag| !tag.is_empty());
                match tag {
                    Some(tag) => SettingValue::Text(tag),
                    None => {
                        return Err(LicomanError::new(
                            ErrorKind::Unavailable,
                            "The compositor version could not be determined from \
                             its IPC response.",
                            format!("unexpected version output: {response:?}"),
                        ));
                    }
                }
            }
            "hyprland.runtime.active_workspace" => {
                let response = self.request(COMMAND_ACTIVE_WORKSPACE)?;
                // Documented shape: `workspace ID <n> (<name>)`.
                let parsed = response
                    .lines()
                    .next()
                    .and_then(|line| {
                        let start = line.find('(')?;
                        let end = line.rfind(')')?;
                        (start < end).then(|| line[start + 1..end].to_string())
                    })
                    .filter(|name| !name.is_empty());
                match parsed {
                    Some(name) => SettingValue::Text(name),
                    None => {
                        return Err(LicomanError::new(
                            ErrorKind::Unavailable,
                            "The active workspace could not be determined from \
                             the IPC response.",
                            format!("unexpected activeworkspace output: {response:?}"),
                        ));
                    }
                }
            }
            "hyprland.runtime.monitors" => {
                let response = self.request(COMMAND_MONITORS)?;
                // Documented shape: one `Monitor <name> (ID <n>):` line per
                // output. Names are listed; a missing marker is an error.
                let names: Vec<String> = response
                    .lines()
                    .filter_map(|line| {
                        let rest = line.trim().strip_prefix("Monitor ")?;
                        let end = rest.find(" (ID")?;
                        Some(rest[..end].trim().to_string())
                    })
                    .collect();
                if names.is_empty() {
                    return Err(LicomanError::new(
                        ErrorKind::Unavailable,
                        "No monitor was reported by the compositor IPC.",
                        format!("unexpected monitors output: {response:?}"),
                    ));
                }
                SettingValue::Text(names.join(", "))
            }
            other => {
                return Err(LicomanError::new(
                    ErrorKind::Unavailable,
                    "This diagnostic is not provided by the compositor backend.",
                    format!("unknown hyprland row {other:?}"),
                ));
            }
        };
        Ok(value)
    }
}

impl Provider for HyprlandDiagnostics {
    fn id(&self) -> &'static str {
        "hyprland-runtime-diagnostics"
    }

    fn probe(&self) -> ProbeResult {
        match self.socket_path() {
            Some(_path) => ProbeResult::available(crate::provider::Capabilities {
                read: true,
                write: false,
                watch: false,
            }),
            None => ProbeResult::unavailable(
                "Hyprland's IPC socket is not available (the compositor is not \
                 running or the session variables are missing), so compositor \
                 diagnostics are not offered.",
            ),
        }
    }

    fn descriptors(&self) -> Vec<SettingDescriptor> {
        let runtime_note = Some(
            "Runtime only — read live from the compositor's documented IPC; \
             never written and not persisted anywhere."
                .to_string(),
        );
        let rows: [(&str, &str); 3] = [
            ("hyprland.runtime.version", "Compositor version"),
            ("hyprland.runtime.active_workspace", "Active workspace"),
            ("hyprland.runtime.monitors", "Monitors"),
        ];
        let mut descriptors: Vec<SettingDescriptor> = rows
            .into_iter()
            .map(|(id, label)| {
                let mut descriptor = SettingDescriptor::new(
                    id,
                    Scope::Desktop,
                    // Own category: the configuration module's rows (the
                    // "Hyprland" category) stay separate from these live
                    // runtime facts, which are always first-class here.
                    "Compositor",
                    label,
                    "Live runtime fact from the Hyprland compositor IPC.",
                    SettingType::Text,
                    Control::ReadOnly,
                );
                descriptor.note = runtime_note.clone();
                descriptor.apply_mode = crate::descriptor::ApplyMode::Staged;
                descriptor
            })
            .collect();
        // Stable display order.
        descriptors.sort_by(|a, b| a.id.cmp(&b.id));
        descriptors
    }

    fn read(&self, id: &str) -> Result<SettingValue, LicomanError> {
        self.read_row(id)
    }

    fn read_detailed(&self, id: &str) -> Result<crate::read::ReadResult, LicomanError> {
        let value = self.read_row(id)?;
        Ok(crate::read::ReadResult {
            value,
            origin: crate::read::ValueOrigin::Runtime,
            writable: crate::read::Writable::No {
                reason: "Compositor diagnostics are read-only.".to_string(),
            },
            freshness: crate::read::Freshness::now(crate::read::fnv1a_fingerprint(id.as_bytes())),
            restart_effect: crate::read::RestartEffect::None,
            details: crate::read::SourceDetails {
                backend_kind: "hyprland-ipc",
                target: id.to_string(),
                origin_note: Some("live compositor IPC response".to_string()),
                imported: false,
                diagnostics: Vec::new(),
            },
        })
    }

    fn read_latency(&self, _id: &str) -> crate::provider::ReadLatency {
        // Real socket I/O: always off the UI thread.
        crate::provider::ReadLatency::Background
    }

    fn write(&self, id: &str, _value: &SettingValue) -> Result<(), LicomanError> {
        Err(LicomanError::new(
            ErrorKind::WriteFailed,
            "Compositor diagnostics are read-only. Licoman does not rewrite \
             Hyprland configuration in this phase.",
            format!("diagnostic write refused for {id:?}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// A tiny documented-IPC stand-in: accepts one connection per queued
    /// (command → response) expectation and closes on EOF.
    struct FixtureIpc {
        path: PathBuf,
    }

    impl FixtureIpc {
        fn new(responses: Vec<(&'static str, &'static str)>) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "licoman-hypr-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(".socket.sock");
            let listener = UnixListener::bind(&path).unwrap();
            std::thread::spawn(move || {
                for (command, response) in responses {
                    let Some(Ok(mut stream)) = listener.incoming().next() else {
                        break;
                    };
                    let mut buffer = String::new();
                    let _ = std::io::Read::read_to_string(&mut stream, &mut buffer);
                    if buffer.trim_end_matches('\n') == command {
                        let _ = stream.write_all(response.as_bytes());
                    }
                    // Close (shutdown happens on drop).
                }
            });
            // The listener lives on inside the serving thread; the handle
            // only cleans the directory up.
            Self { path }
        }
    }

    impl Drop for FixtureIpc {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.path.parent().unwrap());
        }
    }

    fn provider(ipc: &FixtureIpc) -> HyprlandDiagnostics {
        HyprlandDiagnostics::with_socket(ipc.path.clone())
    }

    #[test]
    fn reads_version_active_workspace_and_monitors_from_documented_ipc() {
        let ipc = FixtureIpc::new(vec![
            (
                COMMAND_VERSION,
                "Hyprland, built from branch main at commit abc (2025-01-01).\nTag: v0.56.2, commits: 0\n",
            ),
            (COMMAND_ACTIVE_WORKSPACE, "workspace ID 3 (3:web)\n\n"),
            (
                COMMAND_MONITORS,
                "Monitor eDP-1 (ID 0):\n\t1920x1080@60.00Hz at 0x0\nMonitor DP-5 (ID 1):\n\t2560x1440@144.00Hz at 1920x0\n\n",
            ),
        ]);
        let provider = provider(&ipc);
        assert_eq!(
            provider.read("hyprland.runtime.version").unwrap(),
            SettingValue::Text("0.56.2".into())
        );
        assert_eq!(
            provider.read("hyprland.runtime.active_workspace").unwrap(),
            SettingValue::Text("3:web".into())
        );
        assert_eq!(
            provider.read("hyprland.runtime.monitors").unwrap(),
            SettingValue::Text("eDP-1, DP-5".into())
        );
    }

    #[test]
    fn unexpected_output_is_an_error_not_a_guess() {
        let ipc = FixtureIpc::new(vec![(COMMAND_VERSION, "gibberish")]);
        let error = provider(&ipc).read("hyprland.runtime.version").unwrap_err();
        assert!(error.user_message.contains("could not be determined"));
    }

    #[test]
    fn absent_socket_is_unavailable_and_writes_are_refused() {
        let provider = HyprlandDiagnostics::with_socket(
            std::env::temp_dir().join("licoman-hypr-no-such-socket.sock"),
        );
        assert!(matches!(
            provider.probe().status,
            crate::provider::ProbeStatus::Unavailable { .. }
        ));
        let error = provider.read("hyprland.runtime.version").unwrap_err();
        assert!(error.user_message.contains("not available"));
        let error = provider
            .write("hyprland.runtime.version", &SettingValue::Text("x".into()))
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::WriteFailed);
    }

    #[test]
    fn descriptors_are_read_only_runtime_labeled() {
        let provider = HyprlandDiagnostics::new();
        let descriptors = provider.descriptors();
        assert_eq!(descriptors.len(), 3);
        for descriptor in &descriptors {
            assert_eq!(descriptor.control, Control::ReadOnly);
            assert!(descriptor.note.as_deref().unwrap().contains("Runtime only"));
            assert!(descriptor.id.starts_with("hyprland.runtime."));
        }
    }
}
