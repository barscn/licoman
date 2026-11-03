//! Licoman product preferences — the only configuration Licoman is allowed to
//! write. Stored at `$XDG_CONFIG_HOME/licoman/preferences.toml` (fallback
//! `$HOME/.config/licoman/preferences.toml`). This module is deliberately
//! separate from the registry and provider settings: it models Licoman's own
//! configuration, not the user's desktop (02_FOUNDATION.md §3).
//!
//! Contract (01_DESIGN_SYSTEM.md §2):
//! - Absent or invalid file means **Dark**. A corrupt file must not prevent
//!   startup, and the file is only created after the user changes a preference.

use std::fmt;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;

/// User-selectable application theme. `System` is a Licoman-level choice that
/// resolves at runtime to Dark or Light via the desktop appearance signal;
/// gpui's `ThemeMode` itself only knows Light/Dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppearanceMode {
    #[default]
    Dark,
    Light,
    System,
}

impl AppearanceMode {
    pub const ALL: [AppearanceMode; 3] = [
        AppearanceMode::Dark,
        AppearanceMode::Light,
        AppearanceMode::System,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            AppearanceMode::Dark => "dark",
            AppearanceMode::Light => "light",
            AppearanceMode::System => "system",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AppearanceMode::Dark => "Dark",
            AppearanceMode::Light => "Light",
            AppearanceMode::System => "System",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "dark" => Some(AppearanceMode::Dark),
            "light" => Some(AppearanceMode::Light),
            "system" => Some(AppearanceMode::System),
            _ => None,
        }
    }
}

impl fmt::Display for AppearanceMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preferences {
    pub schema_version: u32,
    pub theme: AppearanceMode,
}

impl Default for Preferences {
    /// Fresh install: **Dark**, regardless of the desktop's appearance.
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            theme: AppearanceMode::Dark,
        }
    }
}

/// Non-fatal diagnostics produced while loading. Surfaced in Preferences, never
/// shown as a blocker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadDiagnostic {
    /// File did not parse as TOML.
    Corrupt,
    /// Parsed, but the schema version is not one this build understands.
    UnsupportedSchema { found: u32 },
    /// `appearance.theme` held an unrecognized string.
    InvalidTheme { found: String },
}

impl LoadDiagnostic {
    pub fn message(&self) -> String {
        match self {
            LoadDiagnostic::Corrupt => {
                "Preferences file could not be read; using Dark.".to_string()
            }
            LoadDiagnostic::UnsupportedSchema { found } => {
                format!("Preferences schema v{found} is not supported by this build; using Dark.")
            }
            LoadDiagnostic::InvalidTheme { found } => {
                format!("Unknown theme \"{found}\" in preferences; using Dark.")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedPreferences {
    pub preferences: Preferences,
    pub diagnostics: Vec<LoadDiagnostic>,
}

impl LoadedPreferences {
    fn pure(preferences: Preferences) -> Self {
        Self {
            preferences,
            diagnostics: Vec::new(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PreferencesFile {
    schema_version: u32,
    appearance: AppearanceSection,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AppearanceSection {
    theme: String,
}

/// Parse preferences from TOML text. Pure — unit tested without the filesystem.
pub fn parse_preferences(text: &str) -> LoadedPreferences {
    let file: PreferencesFile = match toml::from_str(text) {
        Ok(file) => file,
        Err(_) => {
            return LoadedPreferences {
                preferences: Preferences::default(),
                diagnostics: vec![LoadDiagnostic::Corrupt],
            };
        }
    };

    let mut diagnostics = Vec::new();
    if file.schema_version != SCHEMA_VERSION {
        diagnostics.push(LoadDiagnostic::UnsupportedSchema {
            found: file.schema_version,
        });
    }
    let theme = match AppearanceMode::parse(&file.appearance.theme) {
        Some(theme) => theme,
        None => {
            diagnostics.push(LoadDiagnostic::InvalidTheme {
                found: file.appearance.theme.clone(),
            });
            AppearanceMode::Dark
        }
    };

    LoadedPreferences {
        preferences: Preferences {
            // An unsupported schema still yields safe defaults rather than a
            // crash; the diagnostic tells the user what happened.
            schema_version: file.schema_version,
            theme,
        },
        diagnostics,
    }
}

/// Serialize preferences to TOML text.
pub fn serialize_preferences(prefs: &Preferences) -> String {
    let file = PreferencesFile {
        schema_version: prefs.schema_version,
        appearance: AppearanceSection {
            theme: prefs.theme.as_str().to_string(),
        },
    };
    // The struct is infallibly serializable; unwrap is documented as a
    // programmer error if it ever fires.
    toml::to_string_pretty(&file).expect("preferences serialization cannot fail")
}

/// Resolve the preferences directory: `$XDG_CONFIG_HOME/licoman`, falling back
/// to `$HOME/.config/licoman` per the XDG Base Directory Specification.
pub fn config_dir() -> PathBuf {
    config_dir_for(
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

pub fn preferences_path() -> PathBuf {
    config_dir().join("preferences.toml")
}

/// Resolve the Licoman data directory: `$XDG_DATA_HOME/licoman`, falling
/// back to `$HOME/.local/share/licoman` per the XDG Base Directory
/// Specification. Hosts the write engine's bounded private backups (D48).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
    {
        return dir.join("licoman");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(".local/share")
        .join("licoman")
}

fn config_dir_for(xdg: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = xdg
        && dir.is_absolute()
    {
        return dir.join("licoman");
    }
    // A relative XDG_CONFIG_HOME is ignored per spec; fall through to $HOME.
    home.unwrap_or_else(|| PathBuf::from("/"))
        .join(".config")
        .join("licoman")
}

/// Load preferences from `path`. Missing file → defaults (Dark), no diagnostic.
/// Any I/O or parse problem degrades to defaults plus a diagnostic.
pub fn load_from(path: &Path) -> LoadedPreferences {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_preferences(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            LoadedPreferences::pure(Preferences::default())
        }
        Err(_) => LoadedPreferences {
            preferences: Preferences::default(),
            diagnostics: vec![LoadDiagnostic::Corrupt],
        },
    }
}

pub fn load() -> LoadedPreferences {
    load_from(&preferences_path())
}

/// Persist preferences. Only called after a user changes a preference, so the
/// file (and its directory) are created on demand.
pub fn save_to(path: &Path, prefs: &Preferences) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serialize_preferences(prefs);
    // Write to a sibling temp file, then rename: a crash mid-write cannot leave
    // a half-written preferences.toml behind.
    let tmp = tmp_sibling(path);
    std::fs::write(&tmp, text)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn tmp_sibling(path: &Path) -> PathBuf {
    // Single-writer app: a pid-scoped temp name cannot collide between runs.
    path.with_extension(format!("toml.tmp-{}", std::process::id()))
}

pub fn save(prefs: &Preferences) -> std::io::Result<()> {
    save_to(&preferences_path(), prefs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_dark() {
        let prefs = Preferences::default();
        assert_eq!(prefs.theme, AppearanceMode::Dark);
        assert_eq!(prefs.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn missing_file_yields_dark_without_diagnostics() {
        let path = unique_test_path("missing");
        let loaded = load_from(&path);
        assert_eq!(loaded.preferences, Preferences::default());
        assert!(loaded.diagnostics.is_empty());
    }

    #[test]
    fn valid_file_roundtrips() {
        let path = unique_test_path("valid");
        let prefs = Preferences {
            schema_version: SCHEMA_VERSION,
            theme: AppearanceMode::System,
        };
        save_to(&path, &prefs).expect("save");
        let loaded = load_from(&path);
        assert_eq!(loaded.preferences, prefs);
        assert!(loaded.diagnostics.is_empty());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "schema_version = 1\n\n[appearance]\ntheme = \"system\"\n"
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn corrupt_file_falls_back_to_dark_with_diagnostic() {
        let loaded = parse_preferences("this is {{{ not toml");
        assert_eq!(loaded.preferences.theme, AppearanceMode::Dark);
        assert_eq!(loaded.diagnostics, vec![LoadDiagnostic::Corrupt]);
    }

    #[test]
    fn invalid_theme_string_falls_back_to_dark_with_diagnostic() {
        let loaded =
            parse_preferences("schema_version = 1\n\n[appearance]\ntheme = \"solarized\"\n");
        assert_eq!(loaded.preferences.theme, AppearanceMode::Dark);
        assert_eq!(
            loaded.diagnostics,
            vec![LoadDiagnostic::InvalidTheme {
                found: "solarized".to_string()
            }]
        );
    }

    #[test]
    fn unsupported_schema_is_reported_but_does_not_crash() {
        let loaded = parse_preferences("schema_version = 99\n\n[appearance]\ntheme = \"light\"\n");
        assert_eq!(loaded.preferences.theme, AppearanceMode::Light);
        assert_eq!(
            loaded.diagnostics,
            vec![LoadDiagnostic::UnsupportedSchema { found: 99 }]
        );
    }

    /// Phase 09 upgrade gate: a preferences file from an older release
    /// (here: schema v0 — the pre-release shape) must migrate best-effort —
    /// the user's theme selection is preserved, the schema gap is reported
    /// honestly, and re-saving normalizes the file to the current schema.
    /// There is no older release in the wild yet; this pins the contract so
    /// the first real schema change cannot silently drop the theme.
    #[test]
    fn older_schema_file_migrates_and_preserves_the_theme() {
        let old = parse_preferences("schema_version = 0\n\n[appearance]\ntheme = \"light\"\n");
        assert_eq!(old.preferences.theme, AppearanceMode::Light);
        assert_eq!(
            old.diagnostics,
            vec![LoadDiagnostic::UnsupportedSchema { found: 0 }]
        );
        // Saving the migrated preferences normalizes the schema version.
        let migrated = Preferences {
            schema_version: SCHEMA_VERSION,
            theme: old.preferences.theme,
        };
        let text = serialize_preferences(&migrated);
        let roundtrip = parse_preferences(&text);
        assert_eq!(roundtrip.preferences, migrated);
        assert!(roundtrip.diagnostics.is_empty());
    }

    #[test]
    fn empty_file_is_corrupt_and_defaults_to_dark() {
        let loaded = parse_preferences("");
        assert_eq!(loaded.preferences.theme, AppearanceMode::Dark);
        assert_eq!(loaded.diagnostics, vec![LoadDiagnostic::Corrupt]);
    }

    #[test]
    fn config_dir_prefers_absolute_xdg() {
        let dir = config_dir_for(
            Some(PathBuf::from("/custom/xdg")),
            Some(PathBuf::from("/home/tester")),
        );
        assert_eq!(dir, PathBuf::from("/custom/xdg/licoman"));
    }

    #[test]
    fn config_dir_falls_back_to_home_config() {
        let dir = config_dir_for(None, Some(PathBuf::from("/home/tester")));
        assert_eq!(dir, PathBuf::from("/home/tester/.config/licoman"));
    }

    #[test]
    fn config_dir_ignores_relative_xdg() {
        let dir = config_dir_for(
            Some(PathBuf::from("relative/path")),
            Some(PathBuf::from("/home/tester")),
        );
        assert_eq!(dir, PathBuf::from("/home/tester/.config/licoman"));
    }

    #[test]
    fn all_modes_parse_round_trip() {
        for mode in AppearanceMode::ALL {
            assert_eq!(AppearanceMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(AppearanceMode::parse("Dark"), None);
    }

    fn unique_test_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir();
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        dir.join(format!("licoman-test-{name}-{pid}-{nanos}.toml"))
    }
}
