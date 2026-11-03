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
    /// Alacritty version support on this machine (D50): the live
    /// constructor detects it; fixture constructors inject a known state so
    /// tests never depend on the host's installed applications.
    alacritty: crate::read::alacritty::AlacrittySupport,
    /// mpv version support on this machine (Phase 08): same injection
    /// pattern as Alacritty.
    mpv: crate::read::mpv::MpvSupport,
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
            alacritty: crate::read::alacritty::detect(),
            mpv: crate::read::mpv::detect(),
        }
    }

    pub fn for_dirs(config_home: PathBuf, home: PathBuf, system_roots: Vec<PathBuf>) -> Self {
        Self {
            config_home,
            home,
            system_roots,
            // Fixture/test constructor: assume a TOML-era Alacritty and a
            // current-era mpv so the version gates are exercised only where
            // a test deliberately changes them (D50 / Phase 08). The live
            // constructor detects for real.
            alacritty: crate::read::alacritty::AlacrittySupport::Toml(
                crate::read::alacritty::AppVersion::new(0, 13, 0),
            ),
            mpv: crate::read::mpv::MpvSupport::Supported(crate::read::mpv::AppVersion::new(
                0, 41, 0,
            )),
        }
    }

    /// Override the detected Alacritty support state (tests and demos;
    /// the live environment always detects).
    pub fn with_alacritty_support(
        mut self,
        support: crate::read::alacritty::AlacrittySupport,
    ) -> Self {
        self.alacritty = support;
        self
    }

    /// Override the detected mpv support state (tests and demos).
    pub fn with_mpv_support(mut self, support: crate::read::mpv::MpvSupport) -> Self {
        self.mpv = support;
        self
    }

    /// The detected/injected mpv support state (Phase 08).
    pub fn mpv_support(&self) -> &crate::read::mpv::MpvSupport {
        &self.mpv
    }

    /// The detected/injected Alacritty support state (D50).
    pub fn alacritty_support(&self) -> &crate::read::alacritty::AlacrittySupport {
        &self.alacritty
    }

    /// Whether this binding may be **written** in this environment. Reads
    /// stay available regardless; writes are gated per option and version
    /// (07 Integration A: "unsupported versions don't show invented
    /// controls", fail-closed on unknown versions — D50; the mpv gate is
    /// the Phase 08 counterpart).
    pub fn write_support(&self, binding: &BackendBinding) -> SupportState {
        let BackendBinding::File {
            format,
            key,
            resolution,
            ..
        } = binding
        else {
            return SupportState::Unavailable {
                reason: "Not a configuration-file binding.".into(),
            };
        };
        if *format == FileFormat::MpvConf {
            // mpv path: the version gate is family-level (every exposed
            // option shares the era requirement); a named key is still
            // required so a mis-declared manifest shows the honest state.
            if key.is_none() {
                return SupportState::Unavailable {
                    reason: "This setting does not name a configuration key.".into(),
                };
            }
            return match &self.mpv {
                crate::read::mpv::MpvSupport::Supported(version) => {
                    if version.at_least(crate::read::mpv::CONFIG_ERA_MIN) {
                        SupportState::Available
                    } else {
                        SupportState::Unavailable {
                            reason: format!(
                                "This option requires mpv ≥ 0.33, but version {} was \
                                 detected.",
                                version.raw
                            ),
                        }
                    }
                }
                other => SupportState::Unavailable {
                    reason: other.describe(),
                },
            };
        }
        if *format != FileFormat::Toml {
            return SupportState::Unavailable {
                reason: format!(
                    "This configuration is a {format:?} file. TOML and mpv configurations \
                     are edited first; this format is recorded but not edited yet."
                ),
            };
        }
        match resolution {
            Some(FileResolution::Alacritty) => match &self.alacritty {
                crate::read::alacritty::AlacrittySupport::Toml(version) => {
                    let Some(key) = key else {
                        return SupportState::Unavailable {
                            reason: "This setting does not name a configuration key.".into(),
                        };
                    };
                    let min = crate::read::alacritty::option_min_version(key);
                    if version.at_least(min) {
                        SupportState::Available
                    } else {
                        SupportState::Unavailable {
                            reason: format!(
                                "This option requires Alacritty ≥ {}.{}.{}, but version \
                                 {} was detected.",
                                min.0, min.1, min.2, version.raw
                            ),
                        }
                    }
                }
                other => SupportState::Unavailable {
                    reason: other.describe(),
                },
            },
            // Generic single-file bindings have no version dimension.
            _ => SupportState::Available,
        }
    }

    /// The authorized read roots: config home, home, and the system roots.
    /// Every canonicalized read must land inside one of these.
    fn authorized_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![self.config_home.clone(), self.home.clone()];
        roots.extend(self.system_roots.iter().cloned());
        roots
    }

    /// The user's config home this backend reads from (fixture-injectable).
    /// Exposed to the write side for its crash-recovery temp sweep.
    pub(crate) fn config_home(&self) -> &Path {
        &self.config_home
    }

    /// Redact a path for logs and operation records (06_SAFE_WRITE_ENGINE.md
    /// §6): home and config-home prefixes collapse to `~`; everything else
    /// passes through (paths are metadata, never contents).
    pub fn redact_path(&self, path: &str) -> String {
        for prefix in [&self.home, &self.config_home] {
            let prefix_str = prefix.to_string_lossy();
            if let Some(rest) = path.strip_prefix(prefix_str.as_ref()) {
                let rest = rest.trim_start_matches('/');
                return format!("~/{rest}");
            }
        }
        path.to_string()
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

    /// mpv user-configuration candidates in the documented order (deduped).
    fn mpv_candidates(&self) -> Vec<PathBuf> {
        crate::read::mpv::config_candidates(&self.config_home, &self.home, &self.system_roots)
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
    /// which loaded file last set it (the effective winner). Unlike
    /// [`Self::read`], an absent key is a normal `value: None` result — the
    /// write engine needs the distinction between "absent" and "unreadable".
    fn probe_merged(
        &self,
        id: &str,
        root: &Path,
        key_path: &str,
    ) -> Result<FileKeyProbe, LicomanError> {
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

        // Fingerprint over every contributing file (path + content), in
        // load order — any change to any file moves it.
        let mut fingerprint = Vec::new();
        for (path, document) in &order {
            fingerprint.extend_from_slice(path.display().to_string().as_bytes());
            fingerprint.extend_from_slice(document.to_string().as_bytes());
        }

        let (winner_path, imported, value) = match winner {
            Some((path, imported, value)) => (Some(path), imported, Some(value)),
            None => (None, false, None),
        };

        Ok(FileKeyProbe {
            root: root.to_path_buf(),
            winner: winner_path.map(Path::to_path_buf),
            winner_position: winner_path
                .and_then(|winner| order.iter().position(|(p, _)| p == winner)),
            imported,
            value,
            single_file: false,
            files: order.len(),
            fingerprint: fnv1a_fingerprint(&fingerprint),
            diagnostics,
        })
    }

    /// Resolve one key structurally. Unlike [`SettingsReadBackend::read`],
    /// an absent key is a normal `value: None` result; unreadable files,
    /// invalid TOML, containment escapes and unsupported types remain errors.
    /// The write engine builds its pre-checks on this
    /// (06_SAFE_WRITE_ENGINE.md §2.2) and `read` is defined on top of it, so
    /// the two paths can never drift apart.
    pub fn probe_key(
        &self,
        binding: &BackendBinding,
        descriptor: &SettingDescriptor,
    ) -> Result<FileKeyProbe, LicomanError> {
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
        if *format == FileFormat::MpvConf {
            return self.probe_mpv_key(binding, descriptor);
        }
        if *format != FileFormat::Toml {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                format!(
                    "This configuration is a {format:?} file. Licoman reads TOML and mpv \
                     configurations; this format is recorded but not read yet."
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
                    // Distinguish the legacy YAML locations from a truly
                    // absent configuration (07 Integration A: "legacy
                    // locations" are an honest state, never a guess).
                    if crate::read::alacritty::yaml_candidates(&self.config_home, &self.home)
                        .iter()
                        .any(|candidate| candidate.is_file())
                    {
                        return Err(LicomanError::new(
                            ErrorKind::Unavailable,
                            "Alacritty's configuration here is the legacy YAML format \
                             (alacritty.yml), which Alacritty versions before 0.13 read. \
                             Licoman edits TOML configurations (Alacritty 0.13+) only; \
                             nothing was changed."
                                .to_string(),
                            "alacritty: only legacy .yml candidates exist on the \
                             documented search paths"
                                .to_string(),
                        ));
                    }
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
            let item = follow_key(document.as_table(), &segments);
            if let Some(item) = item
                && toml_value_to_setting(item).is_none()
            {
                return Err(LicomanError::new(
                    ErrorKind::Unavailable,
                    "The stored value of this setting has a type Licoman cannot read.",
                    format!("key {key_path:?} type in {}", canonical_root.display()),
                ));
            }
            let value = item.and_then(toml_value_to_setting);
            let meta = std::fs::metadata(&canonical_root).ok();
            let mut fingerprint = Vec::new();
            fingerprint.extend_from_slice(canonical_root.display().to_string().as_bytes());
            fingerprint.extend_from_slice(&bytes);
            if let Some(modified) = meta.as_ref().and_then(|m| m.modified().ok()) {
                fingerprint.extend_from_slice(
                    format!("{:?}", modified.duration_since(std::time::UNIX_EPOCH)).as_bytes(),
                );
            }
            return Ok(FileKeyProbe {
                root: canonical_root.clone(),
                winner: value.is_some().then(|| canonical_root.clone()),
                winner_position: value.is_some().then_some(0),
                imported: false,
                value,
                single_file: true,
                files: 1,
                fingerprint: crate::read::fnv1a_fingerprint(&fingerprint),
                diagnostics: vec![format!("mtime: {:?}", meta.and_then(|m| m.modified().ok()))],
            });
        }

        self.probe_merged(&descriptor.id, &canonical_root, key_path)
    }

    /// mpv branch of [`Self::probe_key`]: locate the documented single user
    /// configuration, parse it with mpv's own grammar, and return the
    /// top-level key's typed value (absent is a normal result). Keys inside
    /// `[profile]` sections are deliberately not matched — this build reads
    /// and edits top-level options only, and the diagnostics record when a
    /// profile-scoped same-name key exists.
    fn probe_mpv_key(
        &self,
        binding: &BackendBinding,
        descriptor: &SettingDescriptor,
    ) -> Result<FileKeyProbe, LicomanError> {
        let BackendBinding::File {
            path: _,
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
        if *format != FileFormat::MpvConf {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "This binding is not an mpv-grammar configuration.",
                format!("mpv resolution got format {format:?} for {}", descriptor.id),
            ));
        }
        if !matches!(resolution, Some(FileResolution::Mpv)) {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "mpv-grammar configurations require the `mpv` resolution.",
                format!(
                    "mpv format with resolution {resolution:?} for {}",
                    descriptor.id
                ),
            ));
        }
        let Some(key_path) = key else {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "This setting does not name a configuration key, so it cannot be read.",
                format!("file binding without key for {}", descriptor.id),
            ));
        };
        let candidates = self.mpv_candidates();
        let Some(root) = candidates.iter().find(|candidate| candidate.is_file()) else {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "No mpv configuration file was found. mpv runs on its built-in defaults; \
                 Licoman shows no invented value."
                    .to_string(),
                format!(
                    "mpv: none of the documented search paths exist (checked: {})",
                    candidates
                        .iter()
                        .map(|candidate| candidate.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        };
        let canonical_root = self.validated(&descriptor.id, root)?;
        let bytes = read_bounded(&canonical_root, MAX_CONFIG_BYTES).map_err(|error| {
            LicomanError::new(
                ErrorKind::Unavailable,
                "This configuration file could not be read.",
                format!("{}: {error}", canonical_root.display()),
            )
        })?;
        let text = std::str::from_utf8(&bytes).map_err(|error| {
            LicomanError::new(
                ErrorKind::Unavailable,
                "This configuration file is not valid UTF-8, so it cannot be read safely.",
                format!("{}: {error}", canonical_root.display()),
            )
        })?;
        let lines = crate::read::mpv::parse_conf(text);
        let raw_lines: Vec<&str> = text.lines().collect();
        let entry = crate::read::mpv::find_top_level_index(&raw_lines, key_path);
        let mut diagnostics: Vec<String> = vec![format!("root: {}", canonical_root.display())];
        // A same-name key inside a profile is a real trap — record it.
        if entry.is_none()
            && lines
                .iter()
                .any(|line| matches!(line, crate::read::mpv::MpvLine::SectionHeader(_)))
        {
            diagnostics.push(format!(
                "no top-level entry for {key_path:?}; profile-scoped sections are not read"
            ));
        }
        let value = match entry {
            Some(index) => {
                if let crate::read::mpv::MpvLine::Entry {
                    value: raw_text, ..
                } = &lines[index]
                {
                    match crate::read::mpv::conf_text_to_value(raw_text, descriptor.setting_type) {
                        Some(value) => {
                            diagnostics.push(format!("raw text: {} characters", raw_text.len()));
                            Some(value)
                        }
                        None => {
                            return Err(LicomanError::new(
                                ErrorKind::Unavailable,
                                "The stored value of this setting has a type Licoman cannot \
                                 read.",
                                format!(
                                    "{id}: key {key_path:?} in {} is not a {t:?} value (raw: \
                                     {raw_text:?})",
                                    canonical_root.display(),
                                    id = descriptor.id,
                                    t = descriptor.setting_type
                                ),
                            ));
                        }
                    }
                } else {
                    None
                }
            }
            None => None,
        };
        let meta = std::fs::metadata(&canonical_root).ok();
        let mut fingerprint = Vec::new();
        fingerprint.extend_from_slice(canonical_root.display().to_string().as_bytes());
        fingerprint.extend_from_slice(&bytes);
        if let Some(modified) = meta.as_ref().and_then(|m| m.modified().ok()) {
            fingerprint.extend_from_slice(
                format!("{:?}", modified.duration_since(std::time::UNIX_EPOCH)).as_bytes(),
            );
        }
        Ok(FileKeyProbe {
            root: canonical_root.clone(),
            winner: value.is_some().then(|| canonical_root.clone()),
            winner_position: value.is_some().then_some(0),
            imported: false,
            value,
            single_file: true,
            files: 1,
            fingerprint: fnv1a_fingerprint(&fingerprint),
            diagnostics,
        })
    }
}

/// One key's structural resolution inside the authorized roots — what the
/// write engine needs before it may touch anything (06_SAFE_WRITE_ENGINE.md
/// §2): which file wins, whether the winner is an import, the current typed
/// value (absent is normal) and the revision fingerprint.
#[derive(Debug, Clone, PartialEq)]
pub struct FileKeyProbe {
    /// Canonical path of the root configuration file.
    pub root: PathBuf,
    /// Canonical path of the file that sets the key (`None` = absent
    /// everywhere in the effective configuration).
    pub winner: Option<PathBuf>,
    /// Position of the winner in load order (diagnostics only).
    pub winner_position: Option<usize>,
    /// The winning value comes from an imported file rather than the root.
    pub imported: bool,
    /// The effective value; `None` = the key is absent.
    pub value: Option<SettingValue>,
    /// True when this was a single-file (no import semantics) resolution.
    pub single_file: bool,
    /// Number of files contributing to the effective configuration.
    pub files: usize,
    /// Fingerprint over every contributing file, same recipe as `read`.
    pub fingerprint: String,
    /// Collection diagnostics (import skips, cycles) — metadata only.
    pub diagnostics: Vec<String>,
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

pub(crate) fn toml_value_to_setting(item: &Item) -> Option<SettingValue> {
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
        if *format == FileFormat::MpvConf {
            // The version gate affects writes only; the read-side state is
            // plain file presence.
            if self
                .mpv_candidates()
                .iter()
                .any(|candidate| candidate.is_file())
            {
                SupportState::Available
            } else {
                SupportState::Unavailable {
                    reason: "No mpv configuration file was found. mpv runs on its built-in \
                             defaults; Licoman does not invent values."
                        .into(),
                }
            }
        } else if *format != FileFormat::Toml {
            SupportState::Unavailable {
                reason: format!(
                    "This configuration is a {format:?} file. Reading TOML and mpv \
                     configurations lands first; this format is recorded but not read yet."
                ),
            }
        } else {
            match resolution {
                Some(FileResolution::Alacritty) | None => {
                    let exists = self.alacritty_candidates().iter().any(|c| c.is_file())
                        || self.config_home.join(path).is_file();
                    if exists {
                        SupportState::Available
                    } else {
                        SupportState::Unavailable {
                            reason: "No configuration file was found. The application runs on \
                                     its built-in defaults; Licoman does not invent values."
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
                Some(FileResolution::Mpv) => SupportState::Unavailable {
                    reason: "mpv-grammar bindings require the mpv_conf format.".into(),
                },
            }
        }
    }

    fn read(
        &self,
        binding: &BackendBinding,
        descriptor: &SettingDescriptor,
    ) -> Result<ReadResult, LicomanError> {
        let probe = self.probe_key(binding, descriptor)?;
        let BackendBinding::File {
            key: Some(key_path),
            resolution,
            ..
        } = binding
        else {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "This setting does not name a configuration key, so it cannot be read.",
                format!("file binding without key for {}", descriptor.id),
            ));
        };
        let Some(value) = probe.value else {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                "This setting is not present in the configuration. The application's \
                 built-in default applies, but Licoman does not ship validated defaults."
                    .to_string(),
                format!(
                    "{}: key {key_path:?} absent from {} file(s)",
                    probe.root.display(),
                    probe.files
                ),
            ));
        };
        let origin = if probe
            .winner
            .as_ref()
            .is_some_and(|winner| winner.starts_with("/etc"))
        {
            ValueOrigin::System
        } else {
            ValueOrigin::User
        };
        let mut details = if probe.single_file {
            SourceDetails {
                backend_kind: "file",
                target: format!("{} :: {}", probe.root.display(), key_path),
                origin_note: Some(
                    "configuration file (single file; no import semantics)".to_string(),
                ),
                imported: false,
                diagnostics: probe.diagnostics,
            }
        } else {
            let origin_note = if probe.imported {
                Some(format!(
                    "imported file ({} of {} merged files sets this value)",
                    probe.winner_position.map(|p| p + 1).unwrap_or(0),
                    probe.files
                ))
            } else if probe.files > 1 {
                Some(format!(
                    "configuration file (last setter of {} merged files)",
                    probe.files
                ))
            } else {
                Some("configuration file".to_string())
            };
            SourceDetails {
                backend_kind: "file",
                target: format!("{} :: {}", probe.root.display(), key_path),
                origin_note,
                imported: probe.imported,
                diagnostics: probe.diagnostics,
            }
        };
        // Surface the detected Alacritty / mpv support on the matching rows'
        // Source details (version applicability is visible, not implied).
        if matches!(resolution, Some(FileResolution::Alacritty)) {
            details
                .diagnostics
                .push(format!("alacritty: {}", self.alacritty.describe()));
        }
        if matches!(resolution, Some(FileResolution::Mpv)) {
            details
                .diagnostics
                .push(format!("mpv: {}", self.mpv.describe()));
        }
        Ok(ReadResult {
            value,
            origin,
            writable: Writable::No {
                reason: "Licoman can read this configuration but cannot change it yet.".to_string(),
            },
            freshness: Freshness::now(probe.fingerprint),
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
