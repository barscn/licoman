# Module trust and permission boundary

**The rule:** modules are data. All execution, authorization and I/O belong
to Licoman's own Rust code. Nothing a manifest says can widen what the
application may do.

## What a module can express (the closed allowlist)

A validated manifest can name, per setting:

| Binding | What it names | What it can never carry |
|---|---|---|
| `mock` | nothing (declared defaults) | — |
| `gsettings` | one `schema` + one `key` | paths, commands, code |
| `file` | one relative path + key, one resolution strategy | absolute paths, `..`, shell metacharacters, URI prefixes |
| `dbus` | one well-known service name, object path, interface, property | command strings, arbitrary interfaces outside the implemented surface |
| `xfconf` | one channel identifier + one `/`-rooted property | shell metacharacters, `..` segments, absolute forms |

Validation (Phase 04 grammar, extended in Phase 08) rejects everything else
with located field-level issues: unknown fields fail loudly, enum values and
control kinds are checked, mock booleans need defaults, and every string
that names a target is checked against an identifier/relative-path grammar.
These strings are data for Licoman's typed backends — they are never
interpolated into any command line anywhere in the codebase.

## Who executes what

- **Reads** run through `crates/core/src/read/` backends: GSettings via
  libgio, config files via bounded canonicalized reads inside approved
  roots, D-Bus via zbus with typed method calls, Xfconf via the documented
  D-Bus API. A manifest selects among these; it cannot add one.
- **Writes** run through the Phase 06/07 safe write engine: policy derived
  by core (a module cannot weaken it), value validation before I/O,
  fingerprint conflict checks, bounded 0600 backups, atomic staging with
  readback verification. The write dispatch is a closed `WriteTarget` enum;
  manifests cannot introduce targets, and explicitly read-only families
  (Xfconf) are refused twice (provider + engine).
- **Authorization** is deny-by-default: there is no privilege broker in
  this build; `required_auth` rows are permission-held and Licoman runs
  unprivileged. No sudo, no polkit bypass, no root GUI.

## Dynamic loading is deliberately not implemented

`~/.local/share/licoman/modules/` (or `$XDG_DATA_HOME/licoman/modules/`) is
**specified but disabled**. Loading arbitrary manifests from user-writable
locations would let anything with write access to the home directory change
what Licoman names as write targets — a real attack surface that requires
its own review before it ships. The conditions that review must demonstrate
(at minimum):

1. A user-controlled but *declarative* loading model — data-only manifests,
   the same closed grammar, no runtime code of any kind.
2. Clear provenance: which user roots are loaded, which settings came from
   them, surfaced in the UI's Source details.
3. No executable plugins: GPUI Kit offers an optional JavaScript runtime —
   Licoman will not enable it. Rust code owns backend execution and
   authorization, always.
4. A documented trust decision for who may add a module that *names write
   targets* (user-level files can name targets even though they cannot
   execute; the engine's gates still apply at write time).

Until that review, the only trusted set is the compile-time-embedded
`modules/builtin/` catalog.

## Threat-model anchors

- Manifests can never widen permissions: the write policy is **derived by
  core** from the binding family and risk, and the module's `policy` field
  can only make a setting stricter.
- No shell anywhere: manifest validation rejects shell-shaped strings, and
  the only spawned processes in the codebase are documented version probes
  (`alacritty --version`, `mpv --version` — direct exec, bounded, no shell)
  plus harness-only service starts on disposable private buses.
- No network execution, no third-party runtime code download, no automatic
  package installation — in tests or at runtime.
- Secrets never log: the operation log carries ids, redacted targets and
  outcomes; the planted-secret test stays green.
