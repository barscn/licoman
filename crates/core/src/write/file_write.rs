//! The structured-configuration-file mutation path (06_SAFE_WRITE_ENGINE.md
//! §2) — **fixture-only in this phase**: the roots come from injected
//! fixture directories, never from the live host.
//!
//! The pipeline for one write, in order:
//!
//! 1. Resolve the target file through the *read* backend's own logic
//!    (approved roots, search paths, import merge) — never from raw
//!    manifest-provided absolute paths.
//! 2. Reject hostile shapes before any content access: path escapes and
//!    symlinks outside the roots (inherited from the read backend), a
//!    symlink **at** the target path itself, non-regular files, and files
//!    not owned by the current user.
//! 3. Re-read the original and compare revision (fingerprint) **and**
//!    expected semantic value. Either mismatch aborts with a conflict —
//!    Licoman never overwrites external changes by default.
//! 4. Refuse to edit a value that an *imported* file wins — imported files
//!    have no module-owned provenance; the UI offers manual resolution
//!    instead.
//! 5. Patch the minimum key(s) with `toml_edit`, preserving comments,
//!    whitespace, ordering and unrelated sections; the edited key's own
//!    trailing comment is preserved too.
//! 6. Back up **only the affected file** under a private backup root
//!    (`0700` dirs, `0600` files), bounded retention.
//! 7. Stage into a securely created temp file on the same filesystem
//!    (`O_EXCL`, `0600`), validate the staged document, restore the
//!    original mode, `fsync`, atomically `rename`, `fsync` the parent
//!    directory. Stale temp files from a previous crash are swept first.
//! 8. Read back through the same read path and verify. On mismatch, restore
//!    the backup and report honestly whether recovery succeeded.
//!
//! Platform assumptions (documented per 06_SAFE_WRITE_ENGINE.md §2.6):
//! POSIX `rename` is atomic within one filesystem (the temp file is created
//! in the target's directory); mode preservation uses Unix permission bits;
//! ownership preservation relies on the check in step 2 (the file must
//! already belong to the current user). ACLs/xattrs are **not** preserved by
//! a rename-replacement and cannot be read with the standard library — this
//! is a documented limitation; the real-write phase (07) must either
//! preserve them or refuse such files before any live write happens.

use std::io::Write as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use toml_edit::{DocumentMut, Item, Table, Value};

use crate::descriptor::SettingDescriptor;
use crate::error::{ErrorKind, LicomanError};
use crate::modules::def::BackendBinding;
use crate::read::SupportState;
use crate::read::file::{FileKeyProbe, FileReadBackend, MAX_CONFIG_BYTES, toml_value_to_setting};
use crate::value::SettingValue;
use crate::write::request::{Change, ChangeRequest, WriteTarget};
use crate::write::{ApplyOutcome, Persistence, conflict, io_error_to_licoman};

/// Backups kept per affected target before the oldest is pruned.
pub const MAX_BACKUPS_PER_TARGET: usize = 10;

/// Extended-attribute operations used by the D53 preserve-or-refuse gate.
/// POSIX ACLs are extended attributes, so preserving xattrs preserves ACLs.
/// Injectable so the refusal path is testable.
pub trait XattrOps: Send + Sync {
    /// All readable `(name, value)` attributes of `path`. Any failure is a
    /// refusal, never a silent skip.
    fn collect(&self, path: &Path) -> std::io::Result<Vec<(std::ffi::OsString, Vec<u8>)>>;
    /// Re-apply the collected attributes to `path` (the staged temp file).
    /// Every attribute must land or the caller refuses the write.
    fn restore(&self, path: &Path, attrs: &[(std::ffi::OsString, Vec<u8>)]) -> std::io::Result<()>;
}

/// The real operations over the `xattr` crate (a thin syscall wrapper).
pub struct RealXattrOps;

impl XattrOps for RealXattrOps {
    fn collect(&self, path: &Path) -> std::io::Result<Vec<(std::ffi::OsString, Vec<u8>)>> {
        let mut out = Vec::new();
        for name in xattr::list(path)? {
            let Some(value) = xattr::get(path, &name)? else {
                continue;
            };
            out.push((name, value));
        }
        Ok(out)
    }

    fn restore(&self, path: &Path, attrs: &[(std::ffi::OsString, Vec<u8>)]) -> std::io::Result<()> {
        for (name, value) in attrs {
            xattr::set(path, name, value)?;
        }
        Ok(())
    }
}

/// Deterministic fault injection (06_SAFE_WRITE_ENGINE.md §7). Tests toggle
/// flags through the shared handle; production constructs a clean set.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct FaultSet {
    /// The staging write fails with `ENOSPC`.
    pub enospc_on_stage: bool,
    /// Creating the backup fails.
    pub fail_backup: bool,
    /// The post-write readback reports a value different from the request.
    pub readback_mismatch: bool,
    /// The emergency restore fails too (no false rollback success).
    pub restore_fails: bool,
}

/// The file write backend over one read backend's approved roots.
pub struct FileWriteBackend {
    read: FileReadBackend,
    backup_root: PathBuf,
    faults: std::sync::Arc<std::sync::Mutex<FaultSet>>,
    /// Current-user uid source (Linux `/proc/self` by default; injectable
    /// so the foreign-ownership rejection is testable).
    uid: Box<dyn Fn() -> u32 + Send + Sync>,
    /// Extended-attribute preserve-or-refuse layer (D53).
    xattrs: Box<dyn XattrOps>,
}

impl FileWriteBackend {
    pub fn new(read: FileReadBackend, backup_root: PathBuf) -> Self {
        Self::with_uid(read, backup_root, Box::new(current_uid))
    }

    /// Injectable uid source (tests).
    pub fn with_uid(
        read: FileReadBackend,
        backup_root: PathBuf,
        uid: Box<dyn Fn() -> u32 + Send + Sync>,
    ) -> Self {
        let backend = Self {
            read,
            backup_root,
            faults: std::sync::Arc::new(std::sync::Mutex::new(FaultSet::default())),
            uid,
            xattrs: Box::new(RealXattrOps),
        };
        // Crash recovery (06_SAFE_WRITE_ENGINE.md §7): temp files left by a
        // process that died before the rename are swept from the backup root
        // and the configuration directories this backend is configured to
        // write (the config home top level and its `alacritty` directory —
        // the known targets of the fixture phase).
        backend.sweep_stale_temps(&backend.backup_root);
        backend.sweep_stale_temps(backend.read.config_home());
        backend.sweep_stale_temps(&backend.read.config_home().join("alacritty"));
        backend.sweep_stale_temps(&backend.read.config_home().join("mpv"));
        backend
    }

    /// Injectable xattr layer (tests assert the D53 refusal paths).
    pub fn with_xattr_ops(mut self, ops: Box<dyn XattrOps>) -> Self {
        self.xattrs = ops;
        self
    }

    /// Test/fault handle.
    pub fn faults(&self) -> std::sync::Arc<std::sync::Mutex<FaultSet>> {
        self.faults.clone()
    }

    /// The read side (root resolution, probing, readback verification).
    pub fn read_backend(&self) -> &FileReadBackend {
        &self.read
    }

    /// Remove leftover `.<name>.licoman-*.tmp` staging files in `dir`.
    /// Called on construction and before every staging run, so a crash
    /// between temp-create and rename can never leave debris behind.
    pub fn sweep_stale_temps(&self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') && name.contains(".licoman-") && name.ends_with(".tmp") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// Execute one file change. The request must already be policy-gated;
    /// this method owns conflict detection, backup, atomic staging and
    /// verification.
    pub(crate) fn apply(
        &self,
        request: &ChangeRequest,
        descriptor: &SettingDescriptor,
    ) -> Result<ApplyOutcome, LicomanError> {
        let started = Instant::now();
        let WriteTargetParts { binding, key_path } = WriteTargetParts::extract(request)?;

        // --- 0: the environment must support editing this option ----------
        // (D50: Alacritty options are version-validated; defense in depth —
        // the provider consults this before rendering writable rows, and the
        // engine refuses independently here.)
        if let SupportState::Unavailable { reason } = self.read.write_support(&binding) {
            return Err(LicomanError::new(
                ErrorKind::Permission,
                reason,
                format!(
                    "write support unavailable for {} ({} :: {key_path})",
                    request.setting_id,
                    binding.kind_name()
                ),
            ));
        }

        // --- 1+2: resolve through approved logic, reject hostile shapes ---
        let probe = self.read.probe_key(&binding, descriptor)?;
        let edit_path = self.edit_target(request, &probe)?;

        // --- 3: revision + semantic conflict check -------------------------
        self.check_preconditions(request, &probe, &key_path)?;

        // --- 4: parse the original (per grammar) ---------------------------
        let original_bytes =
            crate::read::read_bounded(&edit_path, MAX_CONFIG_BYTES).map_err(|error| {
                io_error_to_licoman(&error, "re-reading the original before writing")
            })?;
        let original_text = std::str::from_utf8(&original_bytes).map_err(|error| {
            LicomanError::new(
                ErrorKind::WriteFailed,
                "The configuration file is not valid UTF-8, so it cannot be edited \
                 safely. Nothing was modified.",
                format!("{}: {error}", edit_path.display()),
            )
        })?;

        // --- 5: minimum-key patch, formatting preserved (per grammar) ------
        let is_mpv = matches!(
            &binding,
            BackendBinding::File {
                format: crate::modules::def::FileFormat::MpvConf,
                ..
            }
        );
        let patched = if is_mpv {
            // mpv grammar: patch the single top-level entry line, preserve
            // every other byte.
            let staged = mpv_patch_text(original_text, &key_path, &request.change, request)?;
            validate_staged_mpv(&staged, &key_path, &request.change, request)?;
            staged
        } else {
            let mut document = original_text.parse::<DocumentMut>().map_err(|error| {
                LicomanError::new(
                    ErrorKind::WriteFailed,
                    "The configuration file no longer parses as TOML, so the change \
                     was not applied. Nothing was modified.",
                    format!("{}: {error}", edit_path.display()),
                )
            })?;
            match &request.change {
                Change::Set(value) => {
                    let segments: Vec<&str> = key_path.split('.').collect();
                    set_dotted_key(
                        document.as_table_mut(),
                        &segments,
                        setting_value_to_toml(value).ok_or_else(|| {
                            LicomanError::new(
                                ErrorKind::Validation,
                                "This value's type cannot be represented in the configuration.",
                                format!("unsupported value {value:?} for {}", request.setting_id),
                            )
                        })?,
                    )
                    .map_err(|technical| {
                        LicomanError::new(
                            ErrorKind::WriteFailed,
                            "The configuration could not be edited at the requested key. \
                             Nothing was modified.",
                            format!("{}: {technical}", edit_path.display()),
                        )
                    })?;
                }
                Change::RemoveOverride => {
                    let segments: Vec<&str> = key_path.split('.').collect();
                    remove_dotted_key(document.as_table_mut(), &segments);
                }
            }
            // --- validate the patched document before anything touches disk ---
            let patched = document.to_string();
            validate_staged(&patched, &key_path, &request.change, request)?;
            patched
        };

        if patched == original_text {
            // The desired end state already holds (e.g. removing an override
            // that is already absent): nothing to write, honestly verified.
            crate::log::info(
                "core::write::file",
                "no change needed; target already holds the requested state",
                &[("id", request.setting_id.to_string())],
            );
            return Ok(ApplyOutcome {
                verified: true,
                expect_absent: matches!(request.change, Change::RemoveOverride),
                persistence: Persistence::Persistent,
                restart: request.restart,
                readback_mismatch: None,
                notes: vec!["no change needed".to_string()],
            });
        }

        // --- 6: bounded private backup of only the affected file ----------
        let backup_path = Some(self.create_backup(request, &edit_path, &original_bytes)?);

        // --- 6b: D53 — read the extended attributes (ACLs included) before
        // anything is staged. Any read failure refuses the write up front;
        // an attribute that cannot be restored later also refuses, with the
        // original untouched. (Phase 06 gate: preserve-or-refuse before any
        // live write.)
        let xattrs = self.xattrs.collect(&edit_path).map_err(|error| {
            LicomanError::new(
                ErrorKind::WriteFailed,
                "The file's extended attributes (which include access-control \
                 entries) could not be read, so Licoman refuses to replace this \
                 file — preserving them could not be guaranteed. Nothing was \
                 modified.",
                format!("{}: xattr collect: {error}", edit_path.display()),
            )
        })?;

        // --- 7: secure temp on the same filesystem, atomic rename ---------
        let meta = std::fs::symlink_metadata(&edit_path)
            .map_err(|error| io_error_to_licoman(&error, "reading file metadata"))?;
        // A refusal after the backup was created is safe (the original is
        // untouched); the backup stays for the record but the error says
        // nothing was modified.
        self.stage_and_rename(&edit_path, patched.as_bytes(), &meta, &xattrs)?;

        // --- 8: read back and verify; restore on mismatch -----------------
        match self.verify(request, descriptor, &binding, &key_path) {
            Ok(()) => {
                let elapsed = started.elapsed();
                crate::log::info(
                    "core::write::file",
                    "file change applied and verified",
                    &[
                        ("id", request.setting_id.to_string()),
                        ("elapsed_us", elapsed.as_micros().to_string()),
                    ],
                );
                Ok(ApplyOutcome {
                    verified: true,
                    expect_absent: matches!(request.change, Change::RemoveOverride),
                    persistence: Persistence::Persistent,
                    restart: request.restart,
                    readback_mismatch: None,
                    notes: backup_path
                        .into_iter()
                        .map(|path| format!("backup: {}", path.display()))
                        .collect(),
                })
            }
            Err(verify_error) => Err(self.recover_or_report(
                request,
                backup_path.as_deref(),
                &edit_path,
                verify_error,
                started,
            )),
        }
    }

    /// The file the change must land in. Imported winners are refused: the
    /// module has no provenance over a file it did not name
    /// (06_SAFE_WRITE_ENGINE.md §2.4).
    fn edit_target(
        &self,
        request: &ChangeRequest,
        probe: &FileKeyProbe,
    ) -> Result<PathBuf, LicomanError> {
        if probe.imported {
            return Err(LicomanError::new(
                ErrorKind::Permission,
                "This value comes from an imported configuration file. Licoman will \
                 not edit an imported file it does not own; change it in the \
                 imported file itself or remove the import. Nothing was modified.",
                format!(
                    "{}: winning value in imported file {}",
                    request.setting_id,
                    probe
                        .winner
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                ),
            ));
        }
        let target = probe.root.clone();
        // A symlink at the target path itself is an unwanted shape for
        // writes: rename-replacement would silently replace the user's link
        // with a regular file (06_SAFE_WRITE_ENGINE.md §2.1).
        if let Ok(link_meta) = std::fs::symlink_metadata(&target)
            && link_meta.file_type().is_symlink()
        {
            return Err(LicomanError::new(
                ErrorKind::Permission,
                "The configuration path is a symbolic link. Licoman refuses to \
                 replace links with regular files; nothing was modified.",
                format!("{}: target path is a symlink", target.display()),
            ));
        }
        // Only regular files (never devices, sockets, fifos, directories).
        let meta = std::fs::metadata(&target)
            .map_err(|error| io_error_to_licoman(&error, "inspecting the configuration file"))?;
        if !meta.is_file() {
            return Err(LicomanError::new(
                ErrorKind::Permission,
                "The configuration path is not a regular file, so Licoman will not \
                 write it. Nothing was modified.",
                format!("{}: not a regular file", target.display()),
            ));
        }
        // Unapproved ownership: another user's config is never written.
        let owner = (self.uid)();
        if owner != meta.uid() {
            return Err(LicomanError::new(
                ErrorKind::Permission,
                "This configuration file is owned by another user, so Licoman will \
                 not write it. Nothing was modified.",
                format!(
                    "{}: owner uid {} != current uid {}",
                    target.display(),
                    meta.uid(),
                    owner
                ),
            ));
        }
        Ok(target)
    }

    /// Revision (fingerprint) and expected-semantic-value comparison. Both
    /// must hold or the write aborts with a conflict
    /// (06_SAFE_WRITE_ENGINE.md §2.2).
    fn check_preconditions(
        &self,
        request: &ChangeRequest,
        probe: &FileKeyProbe,
        key_path: &str,
    ) -> Result<(), LicomanError> {
        if let Some(expected) = &request.expected_fingerprint
            && probe.fingerprint != *expected
        {
            return Err(conflict(
                &request.setting_id,
                format!("fingerprint {expected} -> {}", probe.fingerprint),
            ));
        }
        let absent_now = probe.value.is_none();
        match (&request.before, probe.value.as_ref()) {
            (None, None) => Ok(()),
            (Some(before), Some(current)) if before == current => Ok(()),
            (Some(before), Some(current)) => Err(conflict(
                &request.setting_id,
                format!("key {key_path:?} holds {current:?}, expected {before:?}"),
            )),
            (None, Some(current)) => Err(conflict(
                &request.setting_id,
                format!("key {key_path:?} now holds {current:?}; it was absent when read"),
            )),
            (Some(_), None) if absent_now => Err(conflict(
                &request.setting_id,
                format!("key {key_path:?} is absent now; it held a value when read"),
            )),
            (Some(_), None) => unreachable!("absent_now is true when value is None"),
        }
    }

    /// Copy the affected file into the private backup root and prune old
    /// generations. Only this target is copied — never a directory sweep,
    /// never into diagnostics or caches (06_SAFE_WRITE_ENGINE.md §2.7).
    fn create_backup(
        &self,
        request: &ChangeRequest,
        edit_path: &Path,
        original_bytes: &[u8],
    ) -> Result<PathBuf, LicomanError> {
        {
            let faults = self.faults.lock().expect("fault lock");
            if faults.fail_backup {
                return Err(LicomanError::new(
                    ErrorKind::WriteFailed,
                    "The safety backup could not be created, so the change was not \
                     applied. Your original configuration is unchanged.",
                    format!("backup creation fault for {}", request.setting_id),
                ));
            }
        }
        use std::os::unix::fs::PermissionsExt;
        let target_dir = self.backup_root.join(sanitize(&request.setting_id));
        std::fs::create_dir_all(&target_dir).map_err(|error| {
            LicomanError::new(
                ErrorKind::WriteFailed,
                "The safety backup could not be created, so the change was not \
                 applied. Your original configuration is unchanged.",
                format!("backup dir {}: {error}", target_dir.display()),
            )
        })?;
        let _ = std::fs::set_permissions(&target_dir, std::fs::Permissions::from_mode(0o700));
        let name = format!(
            "{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            edit_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "config".to_string())
        );
        let backup_path = target_dir.join(name);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup_path)
            .map_err(|error| {
                LicomanError::new(
                    ErrorKind::WriteFailed,
                    "The safety backup could not be created, so the change was not \
                     applied. Your original configuration is unchanged.",
                    format!("backup file {}: {error}", backup_path.display()),
                )
            })?;
        let _ = std::fs::set_permissions(&backup_path, std::fs::Permissions::from_mode(0o600));
        file.write_all(original_bytes).map_err(|error| {
            let _ = std::fs::remove_file(&backup_path);
            io_error_to_licoman(&error, "writing the safety backup")
        })?;
        file.sync_all().map_err(|error| {
            let _ = std::fs::remove_file(&backup_path);
            io_error_to_licoman(&error, "syncing the safety backup")
        })?;
        prune_backups(&target_dir, MAX_BACKUPS_PER_TARGET);
        Ok(backup_path)
    }

    /// Stage the new content in a securely created temp file in the target's
    /// directory, validate, preserve the mode, restore the extended
    /// attributes (D53 — failure refuses), fsync, rename, fsync the
    /// directory.
    fn stage_and_rename(
        &self,
        edit_path: &Path,
        patched: &[u8],
        original_meta: &std::fs::Metadata,
        xattrs: &[(std::ffi::OsString, Vec<u8>)],
    ) -> Result<(), LicomanError> {
        self.sweep_stale_temps(edit_path.parent().unwrap_or_else(|| Path::new("/")));
        let mut temp_path = edit_path
            .parent()
            .unwrap_or_else(|| Path::new("/"))
            .to_path_buf();
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0);
        let file_name = edit_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "config".to_string());
        temp_path.push(format!(
            ".{file_name}.licoman-{}-{unique}.tmp",
            std::process::id()
        ));
        {
            let faults = self.faults.lock().expect("fault lock");
            if faults.enospc_on_stage {
                // Model ENOSPC striking mid-staging: the temp file exists in
                // a truncated state, then is cleaned up — the original is
                // untouched and no debris remains.
                let _ = std::fs::write(&temp_path, &patched[..patched.len() / 2]);
                let _ = std::fs::remove_file(&temp_path);
                return Err(io_error_to_licoman(
                    &std::io::Error::from(std::io::ErrorKind::StorageFull),
                    "staging the new configuration",
                ));
            }
        }
        let staged = stage_temp(&temp_path, patched).map_err(|error| {
            let _ = std::fs::remove_file(&temp_path);
            io_error_to_licoman(&error, "staging the new configuration")
        })?;
        // Preserve the original mode; never quietly broaden permissions.
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) = staged.set_permissions(std::fs::Permissions::from_mode(
            original_meta.permissions().mode() & 0o7777,
        )) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(io_error_to_licoman(&error, "preserving the file mode"));
        }
        // Restore the extended attributes onto the staged file. A rename
        // replacement would otherwise silently drop them (ACLs included) —
        // any restore failure refuses the whole write (D53).
        if let Err(error) = self.xattrs.restore(&temp_path, xattrs) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(LicomanError::new(
                ErrorKind::WriteFailed,
                "The file's extended attributes (which include access-control \
                 entries) could not be preserved on the replacement, so the \
                 change was refused. Nothing was modified.",
                format!("{}: xattr restore: {error}", edit_path.display()),
            ));
        }
        if let Err(error) = staged.sync_all() {
            let _ = std::fs::remove_file(&temp_path);
            return Err(io_error_to_licoman(&error, "flushing the staged file"));
        }
        drop(staged);
        std::fs::rename(&temp_path, edit_path).map_err(|error| {
            let _ = std::fs::remove_file(&temp_path);
            io_error_to_licoman(&error, "atomically replacing the configuration")
        })?;
        sync_parent(edit_path);
        Ok(())
    }

    /// Read back through the same approved read path and compare with the
    /// request (06_SAFE_WRITE_ENGINE.md §2.8).
    fn verify(
        &self,
        request: &ChangeRequest,
        descriptor: &SettingDescriptor,
        binding: &BackendBinding,
        key_path: &str,
    ) -> Result<(), LicomanError> {
        {
            let faults = self.faults.lock().expect("fault lock");
            if faults.readback_mismatch {
                return Err(LicomanError::new(
                    ErrorKind::Conflict,
                    "After writing, the file does not report the expected value.",
                    format!("readback fault for {}", request.setting_id),
                ));
            }
        }
        let probe = self.read.probe_key(binding, descriptor)?;
        match (&request.change, probe.value.as_ref()) {
            (Change::Set(expected), Some(actual)) if expected == actual => Ok(()),
            (Change::Set(expected), actual) => Err(LicomanError::new(
                ErrorKind::Conflict,
                "After writing, the file does not report the expected value.",
                format!(
                    "{}: key {key_path:?} read back {:?}, expected {:?}",
                    request.setting_id, actual, expected
                ),
            )),
            (Change::RemoveOverride, None) => Ok(()),
            (Change::RemoveOverride, Some(actual)) => Err(LicomanError::new(
                ErrorKind::Conflict,
                "After removing the override, the key is still present in the file.",
                format!(
                    "{}: key {key_path:?} still holds {actual:?}",
                    request.setting_id
                ),
            )),
        }
    }

    /// Restore the backup and report honestly whether recovery succeeded
    /// (06_SAFE_WRITE_ENGINE.md §2.8: never "Saved" without verification,
    /// never a fake rollback success).
    fn recover_or_report(
        &self,
        request: &ChangeRequest,
        backup_path: Option<&Path>,
        edit_path: &Path,
        verify_error: LicomanError,
        started: Instant,
    ) -> LicomanError {
        let restore_failed = self.faults.lock().expect("fault lock").restore_fails;
        let restored = backup_path.is_some()
            && !restore_failed
            && self.restore_from_backup(backup_path.expect("checked is_some"), edit_path);
        crate::log::error(
            "core::write::file",
            "verification failed after write",
            &[
                ("id", request.setting_id.to_string()),
                ("restored", restored.to_string()),
                ("elapsed_us", started.elapsed().as_micros().to_string()),
            ],
        );
        if restored {
            LicomanError::new(
                ErrorKind::WriteFailed,
                "The change was written but did not verify, so Licoman restored \
                 your original configuration from the safety backup. Please review \
                 the file before trying again.",
                format!(
                    "{}: {verify_error}; original restored from backup",
                    request.setting_id
                ),
            )
        } else {
            LicomanError::new(
                ErrorKind::WriteFailed,
                "The change was written but did not verify, and the automatic \
                 restore failed. The file may hold the attempted value — restore \
                 it manually from the safety backup before continuing.",
                format!(
                    "{}: {verify_error}; automatic restore {}",
                    request.setting_id,
                    backup_path
                        .map(|path| format!("failed; backup at {}", path.display()))
                        .unwrap_or_else(|| "unavailable (no backup was created)".into())
                ),
            )
        }
    }

    /// Byte-exact restore of one file from its backup, through the same
    /// staging discipline as a write (including the D53 xattr gate — the
    /// attributes currently on the target are carried over).
    fn restore_from_backup(&self, backup_path: &Path, target: &Path) -> bool {
        let Ok(bytes) = std::fs::read(backup_path) else {
            return false;
        };
        let Ok(meta) = std::fs::symlink_metadata(target) else {
            return false;
        };
        let Ok(xattrs) = self.xattrs.collect(target) else {
            return false;
        };
        self.stage_and_rename(target, &bytes, &meta, &xattrs)
            .is_ok()
    }
}

/// Parts extracted from a request's [`WriteTarget::File`].
pub(crate) struct WriteTargetParts {
    pub binding: BackendBinding,
    pub key_path: String,
}

impl WriteTargetParts {
    pub(crate) fn extract(request: &ChangeRequest) -> Result<Self, LicomanError> {
        let WriteTarget::File {
            path,
            format,
            key,
            resolution,
        } = &request.target
        else {
            return Err(LicomanError::new(
                ErrorKind::WriteFailed,
                "This change is not bound to a configuration file.",
                format!(
                    "file backend got target kind {:?} for {}",
                    request.target.kind_name(),
                    request.setting_id
                ),
            ));
        };
        if key.is_empty() {
            return Err(LicomanError::new(
                ErrorKind::Validation,
                "This setting does not name a configuration key, so it cannot be written.",
                format!("file target without key for {}", request.setting_id),
            ));
        }
        Ok(Self {
            binding: BackendBinding::File {
                path: path.clone(),
                format: *format,
                key: Some(key.clone()),
                resolution: *resolution,
            },
            key_path: key.clone(),
        })
    }
}

/// Set a dotted key, creating any missing intermediate tables, and
/// preserving the existing value's surrounding decor (e.g. a trailing
/// comment) when replacing. New sections render as ordinary `[table]`
/// headers — minimal, valid TOML.
fn set_dotted_key(
    table: &mut toml_edit::Table,
    segments: &[&str],
    value: Value,
) -> Result<(), String> {
    let (first, rest) = segments
        .split_first()
        .ok_or_else(|| "empty key path".to_string())?;
    if rest.is_empty() {
        match table.get_mut(first) {
            Some(Item::Value(old)) => {
                // Preserve the old value's decor (e.g. a trailing comment).
                let mut new_value = value;
                *new_value.decor_mut() = old.decor().clone();
                table.insert(first, Item::Value(new_value));
            }
            Some(_) => {
                return Err(format!(
                    "key {first:?} exists as a table; a value cannot replace it"
                ));
            }
            None => {
                table.insert(first, Item::Value(value));
            }
        }
        return Ok(());
    }
    match table.get_mut(first) {
        Some(Item::Table(inner)) => set_dotted_key(inner, rest, value),
        Some(Item::Value(_)) => Err(format!(
            "key {first:?} holds a value; a nested key cannot live under it"
        )),
        Some(_) => Err(format!("key {first:?} has an unsupported shape")),
        None => {
            let mut inner = Table::new();
            inner.set_implicit(true);
            let result = set_dotted_key(&mut inner, rest, value);
            table.insert(first, Item::Table(inner));
            result
        }
    }
}

/// Remove a dotted key if present. Removing an absent key is a no-op (the
/// desired end state already holds).
fn remove_dotted_key(table: &mut toml_edit::Table, segments: &[&str]) {
    let Some((first, rest)) = segments.split_first() else {
        return;
    };
    if rest.is_empty() {
        table.remove(first);
        return;
    }
    if let Some(Item::Table(inner)) = table.get_mut(first) {
        remove_dotted_key(inner, rest);
    }
}

/// Validate the staged document: it must parse, and the key must now hold
/// the requested value (or be absent for a removal) — all before any disk
/// write (06_SAFE_WRITE_ENGINE.md §2.6 "write into a securely created
/// temporary file …, validate it").
fn validate_staged(
    patched: &str,
    key_path: &str,
    change: &Change,
    request: &ChangeRequest,
) -> Result<(), LicomanError> {
    let document = patched.parse::<DocumentMut>().map_err(|error| {
        LicomanError::new(
            ErrorKind::WriteFailed,
            "The staged configuration does not parse; the change was not applied. \
             Nothing was modified.",
            format!("{}: staged document: {error}", request.setting_id),
        )
    })?;
    let segments: Vec<&str> = key_path.split('.').collect();
    let item = follow_key_pub(document.as_table(), &segments);
    match (change, item) {
        (Change::Set(expected), Some(item)) => {
            let actual = toml_value_to_setting(item);
            if actual.as_ref() != Some(expected) {
                return Err(LicomanError::new(
                    ErrorKind::WriteFailed,
                    "The staged configuration does not hold the requested value; the \
                     change was not applied. Nothing was modified.",
                    format!(
                        "{}: staged key {key_path:?} is {actual:?}",
                        request.setting_id
                    ),
                ));
            }
            Ok(())
        }
        (Change::Set(_), None) => Err(LicomanError::new(
            ErrorKind::WriteFailed,
            "The staged configuration does not hold the requested value; the change \
             was not applied. Nothing was modified.",
            format!("{}: staged key {key_path:?} is missing", request.setting_id),
        )),
        (Change::RemoveOverride, Some(_)) => Err(LicomanError::new(
            ErrorKind::WriteFailed,
            "The staged configuration still holds the override; the change was not \
             applied. Nothing was modified.",
            format!(
                "{}: staged key {key_path:?} still present",
                request.setting_id
            ),
        )),
        (Change::RemoveOverride, None) => Ok(()),
    }
}

/// `toml_edit` free-function mirror of the read side's key walk (used on an
/// owned document).
fn follow_key_pub<'a>(table: &'a toml_edit::Table, segments: &[&str]) -> Option<&'a Item> {
    let (first, rest) = segments.split_first()?;
    let item = table.get(first)?;
    if rest.is_empty() {
        return Some(item);
    }
    match item {
        Item::Table(inner) => follow_key_pub(inner, rest),
        _ => None,
    }
}

fn setting_value_to_toml(value: &SettingValue) -> Option<Value> {
    Some(match value {
        SettingValue::Bool(b) => Value::from(*b),
        SettingValue::Text(text) => Value::from(text.as_str()),
        SettingValue::Integer(n) => Value::from(*n),
        SettingValue::Float(n) => Value::from(*n),
        SettingValue::Choice(choice) => Value::from(choice.as_str()),
        SettingValue::Path(path) => Value::from(path.as_str()),
        SettingValue::List(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(item.as_str());
            }
            Value::from(array)
        }
    })
}

/// Patch one **top-level** entry in an mpv configuration (Phase 08): the
/// entry line is rewritten in place, an absent entry is inserted before the
/// first `[profile]` header (or appended when the file has no sections), and
/// a removal deletes the line. Every other byte of the file is preserved —
/// profile sections, comments and whitespace included. Top-level only: a
/// same-name key inside a profile is never touched (the read side does not
/// see it either).
fn mpv_patch_text(
    original: &str,
    key: &str,
    change: &Change,
    request: &ChangeRequest,
) -> Result<String, LicomanError> {
    use crate::read::mpv::{find_top_level_index, top_level_insert_index};
    let raw_lines: Vec<&str> = original.lines().collect();
    let mut out: Vec<String> = raw_lines.iter().map(|line| line.to_string()).collect();
    match change {
        Change::Set(value) => {
            let conf_text = crate::read::mpv::value_to_conf_text(value).ok_or_else(|| {
                LicomanError::new(
                    ErrorKind::Validation,
                    "This value's type cannot be represented in an mpv configuration.",
                    format!("unsupported value {value:?} for {}", request.setting_id),
                )
            })?;
            let line = format!("{key}={conf_text}");
            match find_top_level_index(&raw_lines, key) {
                Some(index) => out[index] = line,
                None => match top_level_insert_index(&raw_lines) {
                    Some(index) => out.insert(index, line),
                    None => out.push(line),
                },
            }
        }
        Change::RemoveOverride => {
            if let Some(index) = find_top_level_index(&raw_lines, key) {
                out.remove(index);
            }
            // Removing an absent key is a no-op: the desired end state
            // already holds (the caller reports "no change needed").
        }
    }
    let mut patched = out.join("\n");
    if original.ends_with('\n') && !patched.is_empty() {
        patched.push('\n');
    }
    Ok(patched)
}

/// Validate the staged mpv configuration before anything touches disk: the
/// key must now hold the requested value text (or be absent for a removal),
/// parsed with the same grammar the read side uses.
fn validate_staged_mpv(
    patched: &str,
    key: &str,
    change: &Change,
    request: &ChangeRequest,
) -> Result<(), LicomanError> {
    use crate::read::mpv::{MpvLine, classify_raw_line, find_top_level_index};
    let raw_lines: Vec<&str> = patched.lines().collect();
    let expected = match change {
        Change::Set(value) => crate::read::mpv::value_to_conf_text(value).ok_or_else(|| {
            LicomanError::new(
                ErrorKind::Validation,
                "This value's type cannot be represented in an mpv configuration.",
                format!("unsupported value {value:?} for {}", request.setting_id),
            )
        })?,
        Change::RemoveOverride => {
            if find_top_level_index(&raw_lines, key).is_some() {
                return Err(LicomanError::new(
                    ErrorKind::WriteFailed,
                    "The staged configuration still holds the override; the change was not \
                     applied. Nothing was modified.",
                    format!("{}: staged key {key:?} still present", request.setting_id),
                ));
            }
            return Ok(());
        }
    };
    let Some(index) = find_top_level_index(&raw_lines, key) else {
        return Err(LicomanError::new(
            ErrorKind::WriteFailed,
            "The staged configuration does not hold the requested value; the change was \
             not applied. Nothing was modified.",
            format!("{}: staged key {key:?} is missing", request.setting_id),
        ));
    };
    match classify_raw_line(raw_lines[index]) {
        MpvLine::Entry { value, .. } if value == expected => Ok(()),
        other => Err(LicomanError::new(
            ErrorKind::WriteFailed,
            "The staged configuration does not hold the requested value; the change was \
             not applied. Nothing was modified.",
            format!(
                "{}: staged key {key:?} is {other:?}, expected {key}={expected}",
                request.setting_id
            ),
        )),
    }
}

/// `abc.def` → `abc_def` for backup directory names (never a path).
fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn prune_backups(target_dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(target_dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    if files.len() <= keep {
        return;
    }
    // Names carry a nanosecond prefix: lexical order is chronological.
    files.sort();
    let excess = files.len() - keep;
    for path in files.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}

fn stage_temp(temp_path: &Path, bytes: &[u8]) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(temp_path)?;
    file.write_all(bytes)?;
    Ok(file)
}

fn sync_parent(path: &Path) {
    let Some(parent) = path.parent() else {
        return;
    };
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
}

/// The current user's uid via `/proc/self` (Linux; the only platform this
/// build targets). Used for the ownership check — no libc dependency.
pub(crate) fn current_uid() -> u32 {
    std::fs::metadata("/proc/self")
        .map(|meta| {
            use std::os::unix::fs::MetadataExt;
            meta.uid()
        })
        .unwrap_or_else(|_| {
            // If /proc is unavailable, fail conservative: return an impossible
            // uid so the ownership check refuses rather than allows.
            u32::MAX
        })
}
