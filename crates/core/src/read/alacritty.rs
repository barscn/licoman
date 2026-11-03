//! Alacritty version detection and per-option validation (Phase 07,
//! Integration A; decision D50). 07_FIRST_REAL_INTEGRATIONS.md: "Validate
//! real option name/type/default against the detected version before
//! exposing it" and "unsupported versions don't show invented controls".
//!
//! Facts encoded here come from upstream, cited sources:
//!
//! - TOML (`alacritty.toml`) became the configuration format in **0.13.0**
//!   (changelog: "Configuration file now uses TOML instead of YAML"). The
//!   legacy YAML files belong to Alacritty ≤ 0.13 and are never edited.
//! - `cursor.style.blinking` exists since 0.7.0, `window.opacity` since
//!   0.10.0, the `bell` section since 0.5.0 — every option this build
//!   exposes is therefore gated at the TOML era (≥ 0.13.0), and enum value
//!   sets are restricted to values valid across the TOML era (see
//!   [`option_values`]).
//! - `shell` moved to `terminal.shell` in 0.14.0, so it is deliberately
//!   **not** exposed as a file setting (the manifest keeps it mock with that
//!   recorded reason).
//!
//! Detection runs the documented, non-mutating query `alacritty --version`
//! directly (no shell, no arguments beyond the version flag), bounded by a
//! hard timeout, and parses strictly. Any failure is `Unknown`, and
//! `Unknown` never enables writes (fail-closed).

use std::path::PathBuf;
use std::time::Duration;

/// How long `alacritty --version` may take before it is killed and the
/// version is treated as undetectable.
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(3);

/// First TOML-era version: `alacritty.toml` became the configuration in
/// 0.13.0 (upstream changelog).
pub const TOML_ERA_MIN: (u32, u32, u32) = (0, 13, 0);

/// A parsed Alacritty version (`alacritty 0.14.2` → 0.14.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    /// The raw version text as reported (for display).
    pub raw: String,
}

impl AppVersion {
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
            raw: format!("{major}.{minor}.{patch}"),
        }
    }

    pub fn at_least(&self, min: (u32, u32, u32)) -> bool {
        (self.major, self.minor, self.patch) >= min
    }
}

/// Parse the exact output of `alacritty --version`
/// (documented form: `alacritty 0.13.2`). Strict: anything else is
/// undetectable.
pub fn parse_version_output(output: &str) -> Option<AppVersion> {
    let line = output.lines().next()?.trim();
    let mut parts = line.split_whitespace();
    let program = parts.next()?;
    if !program.eq_ignore_ascii_case("alacritty") {
        return None;
    }
    let version = parts.next()?;
    parse_version(version).map(|mut parsed| {
        parsed.raw = version.to_string();
        parsed
    })
}

/// Parse a bare `X.Y.Z` (or `X.Y`) version token.
pub fn parse_version(token: &str) -> Option<AppVersion> {
    let token = token.trim();
    let token = token.split('-').next()?; // tolerate pre-release suffixes
    let mut numbers = [0u32; 3];
    for (index, part) in token.split('.').enumerate() {
        if index > 2 {
            return None;
        }
        numbers[index] = part.parse().ok()?;
    }
    let raw = token.to_string();
    Some(AppVersion {
        major: numbers[0],
        minor: numbers[1],
        patch: numbers[2],
        raw,
    })
}

/// The documented search paths for the **legacy YAML** configuration
/// (`alacritty.yml`, read by Alacritty ≤ 0.13). Used only to produce an
/// honest "legacy configuration" state — never read or edited.
pub fn yaml_candidates(config_home: &std::path::Path, home: &std::path::Path) -> Vec<PathBuf> {
    vec![
        config_home.join("alacritty/alacritty.yml"),
        config_home.join("alacritty.yml"),
        home.join(".config/alacritty/alacritty.yml"),
        home.join(".alacritty.yml"),
    ]
}

/// Minimum version a TOML option requires. Every option exposed by this
/// build is TOML-era; the table keeps the per-option facts in one place so a
/// future option with a higher requirement is a one-line change.
pub fn option_min_version(key: &str) -> (u32, u32, u32) {
    let _ = key;
    TOML_ERA_MIN
}

/// The enum values valid for `key` across the TOML era, per the upstream
/// documentation. `None` = not an enum key. Used to keep manifest-declared
/// options honest: `bell.animation`'s extended easing set (EaseOutSine …)
/// is only documented for current versions, so only the values documented
/// across 0.13–current are exposed.
pub fn option_values(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "window.startup_mode" => Some(&["Windowed", "Maximized", "Fullscreen"][..]),
        "cursor.style.shape" => Some(&["Block", "Underline", "Beam"][..]),
        // `cursor.style.blinking`: Never | Off | On | Always since 0.7.0
        // (default "Off").
        "cursor.style.blinking" => Some(&["Never", "Off", "On", "Always"]),
        // Values documented across the whole TOML era: "Linear" is the
        // default since 0.13.0; "EaseOutFlash" was removed at some point and
        // the extended easing set is newer — so only the overlap is exposed.
        "bell.animation" => Some(&["Ease", "EaseOutQuad", "Linear"][..]),
        _ => None,
    }
}

/// What version detection concluded for this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlacrittySupport {
    /// A TOML-era Alacritty (≥ 0.13.0); options are version-validated.
    Toml(AppVersion),
    /// Detected version predates the TOML configuration (YAML era).
    LegacyYaml(AppVersion),
    /// No binary on `PATH`, the query timed out, or the output did not
    /// parse. Writes are refused (fail-closed, D50).
    Unknown,
}

impl AlacrittySupport {
    /// Human-readable support verdict for status lines and Source details.
    pub fn describe(&self) -> String {
        match self {
            AlacrittySupport::Toml(version) => {
                format!("Alacritty {} (TOML configuration, ≥ 0.13)", version.raw)
            }
            AlacrittySupport::LegacyYaml(version) => format!(
                "Alacritty {} reads the legacy YAML configuration; Licoman edits \
                 TOML configurations (Alacritty 0.13+) only",
                version.raw
            ),
            AlacrittySupport::Unknown => {
                "Alacritty version could not be verified — editing stays disabled \
                 until the version is known"
                    .to_string()
            }
        }
    }

    /// Whether writes to a TOML configuration may be considered at all.
    pub fn writable(&self) -> bool {
        matches!(self, AlacrittySupport::Toml(_))
    }
}

/// Run `alacritty --version` with a hard bound and parse the result.
/// Direct exec of the documented version query — never a shell, never
/// anything else. Any failure is [`AlacrittySupport::Unknown`].
pub fn detect() -> AlacrittySupport {
    match detect_binary("alacritty", VERSION_TIMEOUT) {
        Some(output) => match parse_version_output(&output) {
            Some(version) if version.at_least(TOML_ERA_MIN) => AlacrittySupport::Toml(version),
            Some(version) => AlacrittySupport::LegacyYaml(version),
            None => AlacrittySupport::Unknown,
        },
        None => AlacrittySupport::Unknown,
    }
}

/// Locate a binary on `PATH` and run it with `--version`, bounded. Returns
/// the trimmed stdout. `None` on any failure (absent, spawn error, timeout).
/// Shared with the mpv detector (the same documented `--version` probe
/// shape, direct exec, no shell).
pub(crate) fn detect_binary(program: &str, timeout: Duration) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    let dir = std::env::split_paths(&path).find(|dir| {
        let candidate = dir.join(program);
        candidate.is_file()
    })?;
    let binary = dir.join(program);
    let mut child = std::process::Command::new(&binary)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let (tx, rx) = std::sync::mpsc::channel();
    let piped = child.stdout.take();
    std::thread::Builder::new()
        .name("licoman-version-probe".into())
        .spawn(move || {
            use std::io::Read as _;
            let mut buf = String::new();
            if let Some(mut stdout) = piped {
                let _ = stdout.read_to_string(&mut buf);
            }
            let _ = tx.send(buf);
        })
        .ok()?;
    match rx.recv_timeout(timeout) {
        Ok(output) => {
            let _ = child.wait();
            let trimmed = output.trim().to_string();
            (!trimmed.is_empty()).then_some(trimmed)
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_output_parses_strictly() {
        let parsed = parse_version_output("alacritty 0.13.2\n").expect("parses");
        assert_eq!((parsed.major, parsed.minor, parsed.patch), (0, 13, 2));
        assert_eq!(parsed.raw, "0.13.2");
        assert!(parse_version_output("some other tool 1.0").is_none());
        assert!(parse_version_output("").is_none());
        // Two-token output with trailing info still uses the first line.
        let parsed = parse_version_output("alacritty 0.15.1 (built 2025-01-01)\n").expect("parses");
        assert_eq!(parsed.raw, "0.15.1");
    }

    #[test]
    fn version_tokens_tolerate_prerelease_but_not_garbage() {
        let parsed = parse_version("0.14.0-dev").expect("parses");
        assert_eq!((parsed.major, parsed.minor, parsed.patch), (0, 14, 0));
        assert!(parse_version("zero").is_none());
        assert!(parse_version("1.2.3.4.5").is_none());
    }

    #[test]
    fn era_and_option_gates() {
        let v = AppVersion::new(0, 13, 0);
        assert!(v.at_least(TOML_ERA_MIN));
        assert!(!AppVersion::new(0, 12, 9).at_least(TOML_ERA_MIN));
        assert_eq!(option_min_version("font.size"), TOML_ERA_MIN);
        assert_eq!(
            option_values("cursor.style.blinking"),
            Some(&["Never", "Off", "On", "Always"][..])
        );
        // Only the era-stable easing subset is exposed for bell.animation.
        let bell = option_values("bell.animation").expect("enum");
        assert!(bell.contains(&"Linear"));
        assert!(!bell.contains(&"EaseOutFlash"));
    }

    #[test]
    fn support_states_reflect_writability() {
        assert!(AlacrittySupport::Toml(AppVersion::new(0, 13, 0)).writable());
        assert!(!AlacrittySupport::LegacyYaml(AppVersion::new(0, 12, 9)).writable());
        assert!(!AlacrittySupport::Unknown.writable());
        assert!(AlacrittySupport::Unknown.describe().contains("disabled"));
    }

    #[test]
    fn detection_without_binary_is_unknown() {
        // A name that cannot exist on any PATH yields Unknown without
        // spawning anything and without touching the environment.
        assert_eq!(
            detect_binary("licoman-no-such-binary-xyz", VERSION_TIMEOUT),
            None
        );
    }
}
