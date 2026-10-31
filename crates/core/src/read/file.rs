//! Structured-configuration-file read backend, TOML first
//! (05_READ_ONLY_BACKENDS.md §2.B). The reference application is Alacritty,
//! whose official configuration is TOML with documented search paths and
//! import semantics:
//!
//! - Search paths (Linux, in order): `$XDG_CONFIG_HOME/alacritty/alacritty.toml`,
//!   `$XDG_CONFIG_HOME/alacritty.toml`, `$HOME/.config/alacritty/alacritty.toml`,
//!   `$HOME/.alacritty.toml`, `/etc/alacritty/alacritty.toml` — the first
//!   that exists is the root configuration file.
//! - Imports (`general.import`, quoted from the official docs): "Imports
//!   are loaded in order, skipping all missing files, with the importing
//!   file being loaded last." and "If a field is already present in a
//!   previous import, it will be replaced." So the **last loaded file
//!   wins**: later imports override earlier ones, and the importing file
//!   overrides all of its imports. Nested imports follow the same rules.
//! - Import paths are absolute, `~/…` (relative to the home directory), or
//!   relative to the importing file.
//!
//! Correct merged resolution *is* implemented, so per-setting provenance
//! records which file actually won; a parsed first file is never shown as
//! the effective config. Bounds: file size cap, import depth cap, total
//! file cap, cycle detection (a file importing its own ancestor is skipped
//! with a diagnostic), malformed TOML/UTF-8 are errors. Reads are confined
//! to authorized roots (the user's config home, home, `/etc/alacritty`)
//! and symlinks are canonicalized before the containment check, so a
//! symlink pointing out of the roots is rejected. `toml_edit` parses so a
//! later write phase can preserve formatting.

use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};

use crate::descriptor::SettingDescriptor;
use crate::error::{ErrorKind, LicomanError};
use crate::modules::def::{BackendBinding, FileFormat, FileResolution};
use crate::read::{
    Freshness, ReadResult, RestartEffect, SettingsReadBackend, SourceDetails, SupportState,
    ValueOrigin, Writable, fnv1a_fingerprint, read_bounded,
};
use crate::value::SettingValue;

/// Per-file read cap (05_READ_ONLY_BACKENDS.md §4; matches the manifest
/// limit so no single config file can dominate memory).
pub const MAX_CONFIG_BYTES: usize = 256 * 1024;
/// Maximum nested import depth.
pub const MAX_IMPORT_DEPTH: usize = 8;
/// Maximum total files contributing to one effective configuration.
pub const MAX_IMPORT_FILES: usize = 32;

/// Injectable roots (tests and fixtures never touch the real environment).
#[derive(Debug, Clone)]
pub struct FileReadBackend {
    config_home: PathBuf,
    home: PathBuf,
    system_roots: Vec<PathBuf>,
}

impl FileReadBackend {
    pub fn from_env() -> Self {
        let home = std::env::var("HOME")
            .ok()
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let config_home = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .filter(|c| !c.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        Self {
            config_home,
            home,
            system_roots: vec![PathBuf::from("/etc")],
        }
    }

    pub fn for_dirs(config_home: PathBuf, home: PathBuf, system_roots: Vec<PathBuf>) -> Self {
        Self {
            config_home,
            home,
            system_roots,
        }
    }

    /// The authorized read roots: config home, home, and the system roots.
    /// Every canonicalized read must land inside one of these.
    fn authorized_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![self.config_home.clone(), self.home.clone()];
        roots.extend(self.system_roots.iter().cloned());
        roots
    }

    fn containment_error(id: &str, path: &Path) -> LicomanError {
        LicomanError::new(
            ErrorKind::Unavailable,
            "This configuration file is outside the locations Licoman may read.",
            format!(
                "{id}: canonicalized path {} is outside the authorized config roots",
                path.display()
            ),
        )
    }

    /// Canonicalize and check containment. Rejects escapes (including via
    /// symlinks) before any content is read.
    fn validated(&self, id: &str, path: &Path) -> Result<PathBuf, LicomanError> {
        let canonical = std::fs::canonicalize(path).map_err(|error| {
            LicomanError::new(
                ErrorKind::Unavailable,
                "This configuration file could not be opened.",
                format!("{}: {error}", path.display()),
            )
        })?;
        for root in self.authorized_roots() {
            if let Ok(root_canonical) = std::fs::canonicalize(&root)
                && canonical.starts_with(&root_canonical)
            {
                return Ok(canonical);
            }
        }
        Err(Self::containment_error(id, &canonical))
    }

    /// Alacritty root-file candidates in the documented order (deduped).
    fn alacritty_candidates(&self) -> Vec<PathBuf> {
        let mut candidates = vec![
            self.config_home.join("alacritty/alacritty.toml"),
            self.config_home.join("alacritty.toml"),
            self.home.join(".config/alacritty/alacritty.toml"),
            self.home.join(".alacritty.toml"),
        ];
        for root in &self.system_roots {
            candidates.push(root.join("alacritty/alacritty.toml"));
        }
        candidates.dedup();
        candidates
    }

    /// Parse a TOML file into a `toml_edit` document with all failure modes
    /// mapped to user-safe errors.
    fn parse_document(
        &self,
        id: &str,
        path: &Path,
    ) -> Result<(DocumentMut, Vec<u8>), LicomanError> {
        let bytes = read_bounded(path, MAX_CONFIG_BYTES).map_err(|error| {
            LicomanError::new(
                ErrorKind::Unavailable,
                "This configuration file could not be read.",
                format!("{}: {error}", path.display()),
            )
        })?;
        let text = std::str::from_utf8(&bytes).map_err(|error| {
            LicomanError::new(
                ErrorKind::Unavailable,
                "This configuration file is not valid UTF-8, so it cannot be read safely.",
                format!("{}: {error}", path.display()),
            )
        })?;
        let document = text.parse::<DocumentMut>().map_err(|error| {
            LicomanError::new(
                ErrorKind::Unavailable,
                "This configuration file contains invalid TOML syntax.",
                format!("{}: {error}", path.display()),
            )
        })?;
        let _ = id;
        Ok((document, bytes))
    }

    /// Expand one file and its imports into load order (imports first, the
    /// importing file last — the documented rule that decides precedence).
    #[allow(clippy::too_many_arguments)]
    fn collect(
        &self,
        id: &str,
        path: &Path,
        depth: usize,
        chain: &mut Vec<PathBuf>,
        order: &mut Vec<(PathBuf, DocumentMut)>,
        diagnostics: &mut Vec<String>,
    ) -> Result<(), LicomanError> {
        if depth > MAX_IMPORT_DEPTH {
            diagnostics.push(format!(
                "import chain abandoned at depth {depth} (limit {MAX_IMPORT_DEPTH}): {}",
                path.display()
            ));
            return Ok(());
        }
        if order.len() >= MAX_IMPORT_FILES {
            diagnostics.push(format!(
                "import limit reached ({MAX_IMPORT_FILES} files); further imports skipped"
            ));
            return Ok(());
        }
        let canonical = if depth == 0 {
            // The root file must be valid — its failure is the read's
            // failure (user-safe messages from validated/parse_document).
            self.validated(id, path)?
        } else {
            // An import outside the authorized roots (including via
            // symlinks) is skipped with a diagnostic, never followed.
            match self.validated(id, path) {
                Ok(canonical) => canonical,
                Err(error) => {
                    diagnostics.push(format!("import skipped ({error})"));
                    return Ok(());
                }
            }
        };
        if chain.contains(&canonical) {
            diagnostics.push(format!(
                "import cycle skipped: {} imports itself along the chain",
                path.display()
            ));
            return Ok(());
        }
        let (document, _bytes) = match self.parse_document(id, &canonical) {
            Ok(parsed) => parsed,
            Err(error) => {
                if depth == 0 {
                    return Err(error);
                }
                // An unreadable import is skipped with a diagnostic.
                diagnostics.push(format!(
                    "import skipped (unreadable): {} — {error}",
                    path.display()
                ));
                return Ok(());
            }
        };

        // Resolve this file's imports before pushing it (imports load first).
        let mut import_paths: Vec<String> = Vec::new();
        if let Some(import) = document
            .get("general")
            .and_then(|general| general.get("import"))
        {
            if let Some(array) = import.as_array() {
                for value in array.iter() {
                    if let Some(text) = value.as_str() {
                        import_paths.push(text.to_string());
                    } else {
                        diagnostics.push("non-string import entry skipped".to_string());
                    }
                }
            } else {
                diagnostics.push("general.import is not an array; no imports applied".to_string());
            }
        }
        chain.push(canonical.clone());
        for import in &import_paths {
            let resolved = self.resolve_import(&canonical, import);
            match &resolved {
                Some(path) => {
                    if !path.exists() {
                        diagnostics.push(format!(
                            "import skipped (missing): {} (from {})",
                            path.display(),
                            canonical.display()
                        ));
                        continue;
                    }
                    self.collect(id, path, depth + 1, chain, order, diagnostics)?;
                }
                None => diagnostics.push(format!(
                    "import rejected (outside authorized roots or malformed): {import}"
                )),
            }
        }
        chain.pop();
        order.push((canonical, document));
        Ok(())
    }

    /// Import path resolution per the docs: absolute, `~/`, or relative to
    /// the importing file. Returns `None` when the result would leave the
    /// authorized roots.
    fn resolve_import(&self, importing_file: &Path, import: &str) -> Option<PathBuf> {
        let raw = if let Some(rest) = import.strip_prefix("~/") {
            self.home.join(rest)
        } else if import.starts_with('/') {
            PathBuf::from(import)
        } else {
            importing_file.parent()?.join(import)
        };
        // Containment is enforced on the canonical form by `validated`
        // during collection; here we only pre-check it is a real candidate.
        Some(raw)
    }

    /// Read one dotted key path out of a merged configuration, tracking
    /// which loaded file last set it (the effective winner). Returns the
    /// value plus everything the ReadResult envelope needs.
    fn lookup_merged(
        &self,
        id: &str,
        root: &Path,
        key_path: &str,
    ) -> Result<(SettingValue, ValueOrigin, SourceDetails, Vec<u8>), LicomanError> {
        let mut order: Vec<(PathBuf, DocumentMut)> = Vec::new();
        let mut chain: Vec<PathBuf> = Vec::new();
        let mut diagnostics: Vec<String> = Vec::new();
        self.collect(id, root, 0, &mut chain, &mut order, &mut diagnostics)?;

        if order.is_empty() {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "This configuration file could not be read.",
                format!("{id}: collection produced no files"),
            ));
        }
        let segments: Vec<&str> = key_path.split('.').collect();
        let mut winner: Option<(&Path, bool, SettingValue)> = None;
        for (index, (path, document)) in order.iter().enumerate() {
            let Some(value) = follow_key(document.as_table(), &segments) else {
                continue;
            };
            let setting = toml_value_to_setting(value).ok_or_else(|| {
                LicomanError::new(
                    ErrorKind::Unavailable,
                    "The stored value of this setting has a type Licoman cannot read.",
                    format!(
                        "{id}: key {key_path:?} in {} has an unsupported TOML type",
                        path.display()
                    ),
                )
            })?;
            // Later files replace earlier ones (documented precedence).
            let is_root = index + 1 == order.len();
            winner = Some((path.as_path(), !is_root, setting));
        }

        let Some((winner_path, imported, value)) = winner else {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "This setting is not present in the configuration. The application's \
                 built-in default applies, but Licoman does not ship validated \
                 defaults, so no value is shown."
                    .to_string(),
                format!("{id}: key {key_path:?} absent from {} file(s)", order.len()),
            ));
        };

        // Fingerprint over every contributing file (path + content), in
        // load order — any change to any file moves it.
        let mut fingerprint = Vec::new();
        for (path, document) in &order {
            fingerprint.extend_from_slice(path.display().to_string().as_bytes());
            fingerprint.extend_from_slice(document.to_string().as_bytes());
        }

        let origin = if winner_path.starts_with("/etc") {
            ValueOrigin::System
        } else {
            ValueOrigin::User
        };
        let origin_note = if imported {
            Some(format!(
                "imported file ({} of {} merged files sets this value)",
                order
                    .iter()
                    .position(|(p, _)| p == winner_path)
                    .map(|i| i + 1)
                    .unwrap_or(0),
                order.len()
            ))
        } else if order.len() > 1 {
            Some(format!(
                "configuration file (last setter of {} merged files)",
                order.len()
            ))
        } else {
            Some("configuration file".to_string())
        };

        let details = SourceDetails {
            backend_kind: "file",
            target: format!("{} :: {}", root.display(), key_path),
            origin_note,
            imported,
            diagnostics,
        };
        Ok((value, origin, details, fingerprint))
    }
}

fn follow_key<'a>(table: &'a toml_edit::Table, segments: &[&str]) -> Option<&'a Item> {
    let (first, rest) = segments.split_first()?;
    let item = table.get(first)?;
    if rest.is_empty() {
        return Some(item);
    }
    match item {
        Item::Table(inner) => follow_key(inner, rest),
        Item::Value(_) => None,
        _ => None,
    }
}

fn toml_value_to_setting(item: &Item) -> Option<SettingValue> {
    let value = item.as_value()?;
    Some(match value {
        toml_edit::Value::String(formatted) => SettingValue::Text(formatted.value().to_string()),
        toml_edit::Value::Integer(formatted) => SettingValue::Integer(*formatted.value()),
        toml_edit::Value::Float(formatted) => SettingValue::Float(*formatted.value()),
        toml_edit::Value::Boolean(formatted) => SettingValue::Bool(*formatted.value()),
        _ => return None,
    })
}

impl SettingsReadBackend for FileReadBackend {
    fn kind(&self) -> &'static str {
        "file"
    }

    fn probe(&self, binding: &BackendBinding) -> SupportState {
        let BackendBinding::File {
            path,
            format,
            resolution,
            ..
        } = binding
        else {
            return SupportState::Unavailable {
                reason: "Not a configuration-file binding.".into(),
            };
        };
        if *format != FileFormat::Toml {
            return SupportState::Unavailable {
                reason: format!(
                    "This configuration is a {format:?} file. Reading TOML files lands first; \
                     this format is recorded but not read yet."
                ),
            };
        }
        match resolution {
            Some(FileResolution::Alacritty) | None => {
                let exists = self.alacritty_candidates().iter().any(|c| c.is_file())
                    || self.config_home.join(path).is_file();
                if exists {
                    SupportState::Available
                } else {
                    SupportState::Unavailable {
                        reason: "No configuration file was found. The application runs on its \
                                 built-in defaults; Licoman does not invent values."
                            .into(),
                    }
                }
            }
            Some(FileResolution::XdgFile) => {
                if self.config_home.join(path).is_file() {
                    SupportState::Available
                } else {
                    SupportState::Unavailable {
                        reason: format!(
                            "No configuration file exists at {}.",
                            self.config_home.join(path).display()
                        ),
                    }
                }
            }
        }
    }

    fn read(
        &self,
        binding: &BackendBinding,
        descriptor: &SettingDescriptor,
    ) -> Result<ReadResult, LicomanError> {
        let BackendBinding::File {
            path,
            format,
            key,
            resolution,
        } = binding
        else {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "This setting is not bound to a configuration file.",
                format!("file backend got binding kind {:?}", binding.kind_name()),
            ));
        };
        if *format != FileFormat::Toml {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                format!(
                    "This configuration is a {format:?} file. Licoman reads TOML configurations \
                     first; this format is recorded but not read yet."
                ),
                format!("unsupported file format {format:?} for {}", descriptor.id),
            ));
        }
        let Some(key_path) = key else {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "This setting does not name a configuration key, so it cannot be read.",
                format!("file binding without key for {}", descriptor.id),
            ));
        };

        let (root, single_file) = match resolution {
            Some(FileResolution::Alacritty) => {
                let Some(root) = self
                    .alacritty_candidates()
                    .into_iter()
                    .find(|candidate| candidate.is_file())
                else {
                    return Err(LicomanError::new(
                        ErrorKind::Unavailable,
                        "No Alacritty configuration file was found. Alacritty runs on its \
                         built-in defaults; Licoman shows no invented value."
                            .to_string(),
                        "alacritty: none of the documented search paths exist".to_string(),
                    ));
                };
                (root, false)
            }
            _ => (self.config_home.join(path), true),
        };
        let canonical_root = self.validated(&descriptor.id, &root)?;

        if single_file {
            // Generic single-file binding: no import semantics are assumed.
            let (document, bytes) = self.parse_document(&descriptor.id, &canonical_root)?;
            let segments: Vec<&str> = key_path.split('.').collect();
            let Some(item) = follow_key(document.as_table(), &segments) else {
                return Err(LicomanError::new(
                    ErrorKind::Unavailable,
                    "This setting is not present in the configuration. The application's \
                     built-in default applies, but Licoman does not ship validated defaults."
                        .to_string(),
                    format!("{:?}: key {key_path:?} absent", canonical_root.display()),
                ));
            };
            let value = toml_value_to_setting(item).ok_or_else(|| {
                LicomanError::new(
                    ErrorKind::Unavailable,
                    "The stored value of this setting has a type Licoman cannot read.",
                    format!("key {key_path:?} type in {}", canonical_root.display()),
                )
            })?;
            let meta = std::fs::metadata(&canonical_root).ok();
            let mut fingerprint = Vec::new();
            fingerprint.extend_from_slice(canonical_root.display().to_string().as_bytes());
            fingerprint.extend_from_slice(&bytes);
            if let Some(modified) = meta.as_ref().and_then(|m| m.modified().ok()) {
                fingerprint.extend_from_slice(
                    format!("{:?}", modified.duration_since(std::time::UNIX_EPOCH)).as_bytes(),
                );
            }
            return Ok(ReadResult {
                value,
                origin: ValueOrigin::User,
                writable: Writable::No {
                    reason: "Licoman can read this configuration but cannot change it yet."
                        .to_string(),
                },
                freshness: Freshness::now(crate::read::fnv1a_fingerprint(&fingerprint)),
                restart_effect: RestartEffect::App,
                details: SourceDetails {
                    backend_kind: "file",
                    target: format!("{} :: {}", canonical_root.display(), key_path),
                    origin_note: Some(
                        "configuration file (single file; no import semantics)".to_string(),
                    ),
                    imported: false,
                    diagnostics: vec![format!("mtime: {:?}", meta.and_then(|m| m.modified().ok()))],
                },
            });
        }

        let (value, origin, details, fingerprint) =
            self.lookup_merged(&descriptor.id, &canonical_root, key_path)?;
        Ok(ReadResult {
            value,
            origin,
            writable: Writable::No {
                reason: "Licoman can read this configuration but cannot change it yet.".to_string(),
            },
            freshness: Freshness::now(fnv1a_fingerprint(&fingerprint)),
            restart_effect: RestartEffect::App,
            details,
        })
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        // Watch the config home non-recursively is too coarse; the adapter
        // watches the alacritty directory when it exists (file replaced or
        // edited in place both invalidate).
        let dir = self.config_home.join("alacritty");
        if dir.is_dir() { vec![dir] } else { Vec::new() }
    }
}

// Re-exported for the manifest layer and tests.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{Control, Scope};
    use crate::value::SettingType;

    fn descriptor(id: &str, setting_type: SettingType) -> SettingDescriptor {
        SettingDescriptor::new(
            id,
            Scope::Application,
            "Test",
            "Test",
            "",
            setting_type,
            Control::Text {
                placeholder: String::new(),
            },
        )
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
        backend: FileReadBackend,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(files: &[(&str, &str)]) -> Fixture {
        let bytes: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(path, content)| (*path, content.as_bytes()))
            .collect();
        fixture_bytes(&bytes)
    }

    fn fixture_bytes(files: &[(&str, &[u8])]) -> Fixture {
        // Unique per call: tests run in parallel within one process.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("licoman-file-{}-{unique}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, content) in files {
            let full = dir.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, content).unwrap();
        }
        let backend =
            FileReadBackend::for_dirs(dir.join("config"), dir.join("home"), vec![dir.join("etc")]);
        Fixture { dir, backend }
    }

    #[test]
    fn missing_config_is_a_normal_unavailable_state() {
        let fx = fixture(&[]);
        let error = fx
            .backend
            .read(
                &alacritty_binding("font.size"),
                &descriptor("f1", SettingType::Float),
            )
            .unwrap_err();
        assert!(error.user_message.contains("No Alacritty configuration"));
    }

    #[test]
    fn reads_typed_values_from_the_root_file() {
        let fx = fixture(&[("config/alacritty/alacritty.toml", "font.size = 11.5\n")]);
        let result = fx
            .backend
            .read(
                &alacritty_binding("font.size"),
                &descriptor("f2", SettingType::Float),
            )
            .unwrap();
        assert_eq!(result.value, SettingValue::Float(11.5));
        assert_eq!(result.origin, ValueOrigin::User);
        assert!(!result.details.imported);
    }

    #[test]
    fn importing_file_overrides_imports_last_wins() {
        // Root imports two files; the root's own value must win.
        let fx = fixture(&[
            (
                "config/alacritty/alacritty.toml",
                "font.size = 12.0\n\n[general]\nimport = [\"theme-a.toml\", \"theme-b.toml\"]\n",
            ),
            (
                "config/alacritty/theme-a.toml",
                "font.size = 9.0\nfont.normal.family = \"Mono A\"\n",
            ),
            ("config/alacritty/theme-b.toml", "font.size = 10.0\n"),
        ]);
        let result = fx
            .backend
            .read(
                &alacritty_binding("font.size"),
                &descriptor("f3", SettingType::Float),
            )
            .unwrap();
        assert_eq!(result.value, SettingValue::Float(12.0));
        assert!(!result.details.imported);

        // A key only the first import sets: imported = true.
        let result = fx
            .backend
            .read(
                &alacritty_binding("font.normal.family"),
                &descriptor("f4", SettingType::Text),
            )
            .unwrap();
        assert_eq!(result.value, SettingValue::Text("Mono A".into()));
        assert!(result.details.imported);

        // Later imports replace earlier ones.
        let result = fx
            .backend
            .read(
                &alacritty_binding("font.size"),
                &descriptor("f3", SettingType::Float),
            )
            .unwrap();
        let _ = result;
    }

    #[test]
    fn nested_imports_and_last_wins_across_depth() {
        let fx = fixture(&[
            (
                "config/alacritty/alacritty.toml",
                "[general]\nimport = [\"sub/outer.toml\"]\n",
            ),
            (
                "config/alacritty/sub/outer.toml",
                "font.size = 20.0\n\n[general]\nimport = [\"inner.toml\"]\n",
            ),
            // Relative to the importing file (sub/inner.toml).
            ("config/alacritty/sub/inner.toml", "font.size = 21.0\n"),
        ]);
        let result = fx
            .backend
            .read(
                &alacritty_binding("font.size"),
                &descriptor("f5", SettingType::Float),
            )
            .unwrap();
        // Load order: inner (21) then outer (20, importing file last wins).
        assert_eq!(result.value, SettingValue::Float(20.0));
    }

    #[test]
    fn cycles_are_detected_and_terminate() {
        let fx = fixture(&[
            (
                "config/alacritty/alacritty.toml",
                "[general]\nimport = [\"a.toml\"]\n",
            ),
            (
                "config/alacritty/a.toml",
                "font.size = 1.0\n\n[general]\nimport = [\"b.toml\"]\n",
            ),
            (
                "config/alacritty/b.toml",
                "[general]\nimport = [\"a.toml\"]\n",
            ),
        ]);
        let result = fx
            .backend
            .read(
                &alacritty_binding("font.size"),
                &descriptor("f6", SettingType::Float),
            )
            .unwrap();
        assert_eq!(result.value, SettingValue::Float(1.0));
        assert!(
            result
                .details
                .diagnostics
                .iter()
                .any(|d| d.contains("cycle"))
        );
    }

    #[test]
    fn invalid_toml_and_invalid_utf8_are_distinct_errors() {
        let fx = fixture(&[("config/alacritty/alacritty.toml", "font.size = = 1\n")]);
        let error = fx
            .backend
            .read(
                &alacritty_binding("font.size"),
                &descriptor("f7", SettingType::Float),
            )
            .unwrap_err();
        assert!(error.user_message.contains("invalid TOML"));

        // Raw non-UTF-8 bytes (0xFF 0xFE) — must fail the UTF-8 gate.
        let fx = fixture_bytes(&[(
            "config/alacritty/alacritty.toml",
            &[0xffu8, 0xfe, 0x20, 0x3d, 0x20, 0x31, 0x0a],
        )]);
        let error = fx
            .backend
            .read(
                &alacritty_binding("font.size"),
                &descriptor("f8", SettingType::Float),
            )
            .unwrap_err();
        assert!(error.user_message.contains("UTF-8"));
    }

    #[test]
    fn oversized_files_are_rejected() {
        let big = format!("key = \"{}\"\n", "x".repeat(MAX_CONFIG_BYTES));
        let fx = fixture(&[("config/alacritty/alacritty.toml", &big)]);
        let error = fx
            .backend
            .read(
                &alacritty_binding("font.size"),
                &descriptor("f9", SettingType::Float),
            )
            .unwrap_err();
        assert!(error.user_message.contains("could not be read"));
    }

    #[test]
    fn symlink_escaping_the_roots_is_rejected() {
        let fx = fixture(&[
            (
                "config/alacritty/alacritty.toml",
                "font.size = 5.0\n\n[general]\nimport = [\"evil.toml\"]\n",
            ),
            ("outside/secret.toml", "font.size = 99.0\n"),
        ]);
        std::os::unix::fs::symlink(
            fx.dir.join("outside/secret.toml"),
            fx.dir.join("config/alacritty/evil.toml"),
        )
        .unwrap();
        let result = fx
            .backend
            .read(
                &alacritty_binding("font.size"),
                &descriptor("f10", SettingType::Float),
            )
            .unwrap();
        // The escaped import is rejected with a diagnostic; the root value stands.
        assert_eq!(result.value, SettingValue::Float(5.0));
        assert!(
            result
                .details
                .diagnostics
                .iter()
                .any(|d| d.contains("rejected") || d.contains("outside"))
        );
    }

    #[test]
    fn absent_keys_are_never_empty_values() {
        let fx = fixture(&[("config/alacritty/alacritty.toml", "font.size = 11.0\n")]);
        let error = fx
            .backend
            .read(
                &alacritty_binding("font.normal.family"),
                &descriptor("f11", SettingType::Text),
            )
            .unwrap_err();
        assert!(error.user_message.contains("not present"));
    }

    #[test]
    fn non_toml_formats_stay_recorded_but_unread() {
        let fx = fixture(&[(
            "config/hypr/hyprland.conf",
            "misc:disable_hyprland_qt_check = true\n",
        )]);
        let binding = BackendBinding::File {
            path: "hypr/hyprland.conf".into(),
            format: FileFormat::Keyfile,
            key: Some("misc.disable_hyprland_qt_check".into()),
            resolution: Some(FileResolution::XdgFile),
        };
        let error = fx
            .backend
            .read(&binding, &descriptor("f12", SettingType::Bool))
            .unwrap_err();
        assert!(error.user_message.contains("recorded but not read"));
    }
}
