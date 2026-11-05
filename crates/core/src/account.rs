//! Current-user account facts and own-user changes through AccountsService.
//!
//! Reads (`detect`, avatar lookup) are plain file reads: `/etc/passwd` for
//! the identity and the `~/.face` convention for the picture — they work on
//! any Linux system, with or without the daemon. Changes go through the
//! **AccountsService** daemon on the system bus (`org.freedesktop.Accounts`
//! / `org.freedesktop.Accounts.User`), the same typed path GNOME and KDE
//! settings use: polkit's `change-own-user-data` action covers own-user
//! real-name, icon and password changes from an active session, so the GUI
//! still never asks for a password. Only typed calls on the caller's own
//! user object exist here — no generic call surface, no other users, and the
//! Phase 06 [`crate::write::broker`] stays deny-all for module settings.
//!
//! The client is zbus behind a trait (mockable), using the blocking API on a
//! bounded worker thread exactly like [`crate::read::dbus`].

use std::path::{Path, PathBuf};

use crate::read::dbus::{ClientError, call_bounded};

/// The current user as `/etc/passwd` describes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserAccount {
    pub uid: u32,
    pub username: String,
    /// GECOS full name (first comma field), empty when unset.
    pub full_name: String,
    pub home: PathBuf,
    pub shell: Option<String>,
}

/// One parsed `/etc/passwd` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswdEntry {
    pub username: String,
    pub uid: u32,
    pub full_name: String,
    pub home: PathBuf,
    pub shell: String,
}

/// Parse one passwd line: `name:passwd:uid:gid:gecos:home:shell`.
pub fn parse_passwd_line(line: &str) -> Option<PasswdEntry> {
    if line.trim().is_empty() || line.starts_with('#') {
        return None;
    }
    let mut fields = line.split(':');
    let username = fields.next()?.to_string();
    fields.next()?; // password hash (shadowed)
    let uid = fields.next()?.parse().ok()?;
    let _gid = fields.next()?;
    let gecos = fields.next().unwrap_or("");
    let home = fields.next()?;
    let shell = fields.next().unwrap_or("");
    if username.is_empty() || home.is_empty() {
        return None;
    }
    Some(PasswdEntry {
        username,
        uid,
        // The GECOS full name is the first comma-separated field (office,
        // phone extensions follow). Kept verbatim — display only.
        full_name: gecos.split(',').next().unwrap_or("").trim().to_string(),
        home: PathBuf::from(home),
        shell: shell.to_string(),
    })
}

/// The current user's uid via `/proc/self` metadata (Linux-only target, no
/// libc) — the same technique as the write path's ownership check.
pub fn current_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self")
        .ok()
        .map(|meta| meta.uid())
        .filter(|uid| *uid != u32::MAX)
}

/// Detect the current user from a passwd file body, for `uid`.
pub fn detect_account(uid: u32, passwd_text: &str) -> Option<UserAccount> {
    passwd_text
        .lines()
        .filter_map(parse_passwd_line)
        .find(|entry| entry.uid == uid)
        .map(|entry| UserAccount {
            uid: entry.uid,
            username: entry.username,
            full_name: entry.full_name,
            home: entry.home,
            shell: if entry.shell.is_empty() {
                None
            } else {
                Some(entry.shell)
            },
        })
}

impl UserAccount {
    /// Detect the current user from the live system (`uid` via `/proc/self`,
    /// identity via `/etc/passwd`).
    pub fn detect() -> Option<Self> {
        let uid = current_uid()?;
        let text = std::fs::read_to_string("/etc/passwd").ok()?;
        detect_account(uid, &text)
    }

    /// The first existing avatar candidate: the `~/.face` convention first,
    /// then AccountsService's cached icon. Order is deterministic and tested.
    pub fn first_avatar(&self) -> Option<PathBuf> {
        avatar_candidates(&self.home, &self.username)
            .into_iter()
            .find(|path| path.is_file())
    }
}

/// Avatar candidate paths, in display priority order.
pub fn avatar_candidates(home: &Path, username: &str) -> Vec<PathBuf> {
    vec![
        home.join(".face"),
        home.join(".face.icon"),
        PathBuf::from(format!("/var/lib/AccountsService/icons/{username}")),
    ]
}

// ---- AccountsService client ---------------------------------------------------

/// What the account UI needs from the AccountsService daemon. Mocked in
/// tests; the real client runs every call on the bounded worker thread.
pub trait AccountsClient: Send + Sync {
    /// The user object path for `uid` (`FindUserById`), or an error when the
    /// daemon is unreachable/denies.
    fn find_user_path(&self, uid: u32) -> Result<String, ClientError>;

    /// Read one string property (`RealName`, `IconFile`, `Shell`) on the
    /// user object.
    fn property(&self, path: &str, name: &str) -> Result<String, ClientError>;

    /// Change the full name (`SetRealName`).
    fn set_real_name(&self, path: &str, name: &str) -> Result<(), ClientError>;

    /// Change the password (`SetPassword`, plaintext like libaccountsservice
    /// itself — the system bus is a peer-cred-authenticated local socket).
    fn set_password(&self, path: &str, password: &str) -> Result<(), ClientError>;

    /// Point the account picture at `filename` (`SetIconFile`).
    fn set_icon_file(&self, path: &str, filename: &str) -> Result<(), ClientError>;
}

/// zbus-backed client (production). Every call opens a fresh bounded system
/// bus connection off the UI thread.
#[derive(Debug, Clone, Copy, Default)]
pub struct ZBusAccounts {
    /// `None` = system bus (default). Injectable for tests that run a
    /// private bus.
    pub connect_system: bool,
}

const MANAGER_DEST: &str = "org.freedesktop.Accounts";
const MANAGER_PATH: &str = "/org/freedesktop/Accounts";
const MANAGER_IFACE: &str = "org.freedesktop.Accounts";
const USER_IFACE: &str = "org.freedesktop.Accounts.User";

impl AccountsClient for ZBusAccounts {
    fn find_user_path(&self, uid: u32) -> Result<String, ClientError> {
        use zbus::blocking::Proxy;
        call_bounded("accounts-find-user", move || {
            let connection = zbus::blocking::Connection::system()?;
            let proxy = Proxy::new(&connection, MANAGER_DEST, MANAGER_PATH, MANAGER_IFACE)?;
            let path: zbus::zvariant::OwnedObjectPath =
                proxy.call("FindUserById", &(i64::from(uid)))?;
            Ok(path.to_string())
        })
    }

    fn property(&self, path: &str, name: &str) -> Result<String, ClientError> {
        use zbus::blocking::Proxy;
        let path = path.to_string();
        let name = name.to_string();
        let owned: zbus::zvariant::OwnedValue = call_bounded("accounts-get", move || {
            let connection = zbus::blocking::Connection::system()?;
            let proxy = Proxy::new(
                &connection,
                MANAGER_DEST,
                path,
                "org.freedesktop.DBus.Properties",
            )?;
            proxy.call("Get", &(USER_IFACE, name))
        })?;
        match zbus::zvariant::Value::from(owned) {
            zbus::zvariant::Value::Str(text) => Ok(text.to_string()),
            other => Err(ClientError::Other(format!(
                "Accounts property type: {other:?}"
            ))),
        }
    }

    fn set_real_name(&self, path: &str, name: &str) -> Result<(), ClientError> {
        self.user_call(path, "SetRealName", name)
    }

    fn set_password(&self, path: &str, password: &str) -> Result<(), ClientError> {
        self.user_call(path, "SetPassword", password)
    }

    fn set_icon_file(&self, path: &str, filename: &str) -> Result<(), ClientError> {
        self.user_call(path, "SetIconFile", filename)
    }
}

impl ZBusAccounts {
    /// One typed `Set*` method call on the user object (single-string
    /// argument; `SetPassword`'s hint stays empty like libaccountsservice).
    fn user_call(&self, path: &str, method: &'static str, arg: &str) -> Result<(), ClientError> {
        use zbus::blocking::Proxy;
        let path = path.to_string();
        let arg = arg.to_string();
        call_bounded("accounts-set", move || {
            let connection = zbus::blocking::Connection::system()?;
            let proxy = Proxy::new(&connection, MANAGER_DEST, path, USER_IFACE)?;
            proxy.call::<_, (&str,), ()>(method, &(arg.as_str(),))
        })
    }
}

/// User-safe one-line explanation for an account-service failure (the UI
/// renders this verbatim; the technical part stays in logs).
pub fn user_message(error: &ClientError) -> String {
    // Absence (no bus, no daemon) is classified first — it is a state, not
    // a failure, and the message points at what still works.
    if service_missing(error) {
        return "The accountsservice daemon is not reachable on the system \
                bus, so this change is unavailable. Account information is \
                still read from /etc/passwd and ~/.face; nothing was \
                modified."
            .to_string();
    }
    match error {
        ClientError::Timeout => "The account service did not answer in time. It may be busy; \
             try again later. The change may not have been applied — \
             reopen this panel to check before retrying."
            .to_string(),
        ClientError::Denied(_) => "The system refused this account change. Your session may need \
             to be re-authenticated; log out and back in, then try again."
            .to_string(),
        ClientError::NoBus(_) => "The system message bus is not reachable, so account changes \
             are unavailable. Nothing was modified."
            .to_string(),
        ClientError::Other(_) => "The account service reported an unexpected error. Nothing \
             should have changed; reopen this panel to check."
            .to_string(),
    }
}

/// Honest "daemon absent" classification: a missing system bus or a missing
/// service name is absence, not a denial.
fn service_missing(error: &ClientError) -> bool {
    match error {
        ClientError::NoBus(_) => true,
        ClientError::Other(detail) => {
            detail.contains("service disappeared")
                || detail.contains("ServiceUnknown")
                || detail.contains("NameHasNoOwner")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::dbus::ClientError;

    const PASSWD: &str = "\
root:x:0:0:root:/root:/bin/bash
baris:x:1000:1000:Barışcan Aslan:/home/baris:/usr/bin/zsh
svc:x:900:900::/var/lib/svc:
sshd:x:71:71:sshd privilege separation:/var/empty:/sbin/nologin
# a comment
broken-line-without-enough-fields
";

    #[test]
    fn parses_passwd_lines() {
        let entry =
            parse_passwd_line("baris:x:1000:1000:Barışcan Aslan,/office:/home/baris:/usr/bin/zsh")
                .expect("parsed");
        assert_eq!(entry.username, "baris");
        assert_eq!(entry.uid, 1000);
        assert_eq!(entry.full_name, "Barışcan Aslan");
        assert_eq!(entry.home, PathBuf::from("/home/baris"));
        assert_eq!(entry.shell, "/usr/bin/zsh");
    }

    #[test]
    fn rejects_malformed_passwd_lines() {
        assert!(parse_passwd_line("").is_none());
        assert!(parse_passwd_line("# comment").is_none());
        assert!(parse_passwd_line("broken-line").is_none());
        assert!(parse_passwd_line("a:b:c:not-a-uid:d:e:f").is_none());
        assert!(parse_passwd_line(":x:1:1::/home:/bin/sh").is_none());
    }

    #[test]
    fn detects_account_by_uid() {
        let account = detect_account(1000, PASSWD).expect("found");
        assert_eq!(account.username, "baris");
        assert_eq!(account.full_name, "Barışcan Aslan");
        assert_eq!(account.shell.as_deref(), Some("/usr/bin/zsh"));
        // A uid without an entry is an honest None.
        assert!(detect_account(1999, PASSWD).is_none());
    }

    #[test]
    fn empty_gecos_and_shell_degrade() {
        let entry = parse_passwd_line("svc:x:900:900::/var/lib/svc:").expect("parsed");
        assert_eq!(entry.full_name, "");
        assert_eq!(entry.shell, "");
        let account = detect_account(900, PASSWD).unwrap();
        assert_eq!(account.full_name, "");
        assert_eq!(account.shell, None);
    }

    #[test]
    fn avatar_candidates_in_face_convention_order() {
        let candidates = avatar_candidates(Path::new("/home/baris"), "baris");
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("/home/baris/.face"),
                PathBuf::from("/home/baris/.face.icon"),
                PathBuf::from("/var/lib/AccountsService/icons/baris"),
            ]
        );
    }

    /// The client is exercised through the trait in the UI layer with mocks;
    /// here only the user-safe message mapping is asserted.
    #[test]
    fn user_messages_are_actionable() {
        let missing = ClientError::Other("service disappeared".into());
        let message = user_message(&missing);
        assert!(message.contains("accountsservice"));
        assert!(message.contains("nothing was modified"));

        let timeout = user_message(&ClientError::Timeout);
        assert!(timeout.contains("did not answer in time"));

        let denied = user_message(&ClientError::Denied("denied".into()));
        assert!(denied.contains("refused"));

        let nobus = user_message(&ClientError::NoBus("no socket".into()));
        // NoBus classifies as absence (the daemon is not reachable), same
        // honest message family as a missing service name.
        assert!(nobus.contains("accountsservice"));
    }

    #[test]
    fn service_missing_classifies_absence_only() {
        assert!(service_missing(&ClientError::NoBus("x".into())));
        assert!(service_missing(&ClientError::Other(
            "service disappeared".into()
        )));
        assert!(!service_missing(&ClientError::Denied("x".into())));
        assert!(!service_missing(&ClientError::Timeout));
        assert!(!service_missing(&ClientError::Other(
            "Accounts property type".into()
        )));
    }
}
