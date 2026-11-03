# Contributing a module

This is the beginner workflow for adding a **data-only module** — no Rust
code, no UI changes. The Phase 08 proof is `desktop-gnome-peripherals.toml`,
which was added as manifest + documentation alone.

## The workflow

1. **Pick a supported provider type** and read the current official
   documentation for your target application/DE version:
   - `mock` — declared defaults only (deterministic samples, labelled as
     such in the UI). Good for catalog design and tests.
   - `gsettings` — a GSettings `schema`/`key` pair. The schema and key must
     exist; presence is probed per row, and a missing key is an honest
     unavailable state.
   - `file` — a configuration file under `$XDG_CONFIG_HOME` (relative path).
     TOML is read and edited (with Alacritty's documented import merge via
     `resolution = "alacritty"`); mpv's documented grammar is read and
     edited (`format = "mpv_conf"`, `resolution = "mpv"`). Other formats are
     recorded but not read yet.
   - `dbus` — a service property. In this build only the power-profiles
     service is implemented; other services are recorded but not read.
   - `xfconf` — an Xfconf `channel`/`property` pair (the Xfce configuration
     service on the session bus). **Read-only by design**: rows validate the
     actual channel and property before any value is shown.

   Check `docs/compatibility.md` first — if the backend family does not
   reach your target's version yet, the honest answer is a manifest with
   `policy = "read_only"` and a support level of `partial`/`read_only`, not
   invented controls.

2. **Create namespaced stable IDs.** Module ids must start with their
   scope prefix (`shared.`, `desktop.`, `distro.`, `app.`) and setting ids
   are module-relative and stable — they become part of search, provenance
   and (later) preference storage. Never rename a shipped id.

3. **Define typed settings**: every setting declares a value `type`
   (`boolean`, `enum`, `integer`, `float`, `string`, `path`), a `control`,
   an `apply policy` (`immediate`/`staged`/`read_only`), risk, restart
   semantics and a typed `default`. Strings are never a universal data
   type; only `string`/`path` settings may be `nullable`.

4. **State provenance and apply policy honestly.** `policy = "read_only"`
   requires `control = "read_only"` — the UI must never render a control
   that cannot save. `required_auth` rows are permission-held; Licoman runs
   unprivileged.

5. **Supply fixtures and negative cases** for file-backed modules under
   `tests/fixtures/` (see `tests/fixtures/alacritty/` for the shape:
   versioned representative configs plus the incompatible/legacy variants).
   Backend behaviour is asserted by tests in `crates/core/src/read/` — add
   cases for malformed input, absent keys and external edits.

6. **Run the linter and the tests**:

   ```sh
   cargo run -p licoman -- module validate modules/builtin/   # or your draft file
   cargo test --workspace
   ```

7. **Review the screens** in Dark and Light (`LICOMAN_DEMO=light`),
   keyboard focus, and with a long translated string pasted into a label —
   the design system wraps; it must not truncate beyond retrieval. Check
   that long application names and paths render in the Applications window.

8. **Add a supported-version matrix row** to `docs/compatibility.md`
   (detected versions, tested systems, read/write/persistence status, known
   limits) and cite the upstream documentation in the manifest's
   `docs_url`.

9. **Submit a small reviewable PR** — one module (or one tightly related
   batch), never a mega-commit with unrelated modules.

## Validation rules worth knowing

- Unknown fields fail loudly with line/column — typos never silently drop.
- Shell metacharacters, absolute paths, `..` segments and URI prefixes in
  backend targets are rejected (nothing in a manifest can ever reach a
  shell).
- A module cannot declare itself `unsupported`; a registered module that
  cannot work here is simply inapplicable, with preserved reasons.
- Mock-backed settings of every type must declare a default.
- Duplicate setting ids inside a module are rejected.

## What modules can never do

No commands, no code, no network access, no absolute paths, no privileged
operations. Licoman's Rust code owns all backend execution and
authorization (see [`docs/security/module-trust.md`](../docs/security/
module-trust.md)). If your target needs any of those, it needs a reviewed
native backend first — file an issue describing the documented public API
you have in mind.
