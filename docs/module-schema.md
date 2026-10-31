# Licoman Module Schema — Authoring Guide and Reference

**Status:** as of Phase 05 (`schema_version = 1`). The canonical types live in
`crates/core/src/modules/def.rs`; the published JSON Schema is generated from
them and can be printed with:

```sh
licoman module schema > licoman-module.schema.json
```

## What a module is

A module is a **data-only** TOML file that describes a set of settings:
what they mean, which control represents them, where their values live and
whether they apply to the detected environment. Modules can never execute
code, spawn processes, write host files or elevate privileges:

- Backend bindings are a **closed allowlist** (`mock`, `gsettings`, `dbus`,
  `file`) with typed fields — there is no escape hatch to a command.
- Every backend string is validated against an identifier/relative-path
  grammar; shell metacharacters (`; | & $ \` > < \` etc.) and path escapes
  (`..`, absolute paths, `~`) fail validation.
- Mock backends serve declared defaults; the real backends (`gsettings`,
  `file`, `dbus`) **read effective values with full provenance** through the
  read-only backends (Phase 05 — see `docs/architecture/backends.md`) and
  refuse writes unconditionally (the safe write engine is Phase 06).

## Minimal example

```toml
schema_version = 1
id = "desktop.gnome.interface"
scope = "desktop"
title = "GNOME Interface"
description = "Appearance and behavior settings provided by GNOME."
category = "appearance"
docs_url = "https://docs.gtk.org/gio/class.Settings.html"

[compatibility]
desktops = ["GNOME"]
required_backends = ["gsettings"]

[[settings]]
id = "clock.show_weekday"
title = "Show weekday in clock"
description = "Display the weekday in the top bar clock."
keywords = ["clock", "date", "weekday", "top bar"]
type = "boolean"
control = "switch"
policy = "immediate"

[settings.backend]
kind = "gsettings"
schema = "org.gnome.desktop.interface"
key = "clock-show-weekday"
```

See `modules/examples/` for annotated examples: a mock application, a
GSettings module, a read-only service module and one that intentionally
fails compatibility. Built-in modules live in `modules/builtin/` and are
embedded into the binary at compile time.

## Field reference — module level

| Field | Type | Required | Meaning |
|---|---|---|---|
| `schema_version` | integer | yes | Module file format version. This build supports **1** only; other versions are rejected with an explicit diagnostic. |
| `id` | string | yes | Namespaced, immutable module id. Must start with the scope prefix (`shared.` / `desktop.` / `distro.` / `app.`) and have at least two dot-separated lowercase segments. |
| `scope` | string | yes | `shared`, `desktop`, `distribution` or `application`. `application` modules surface in the Applications window only. |
| `title` | string | yes | 1–200 characters. |
| `description` | string | no | Up to 1000 characters. |
| `category` | string | yes | Navigation grouping (1–64 chars); groups the module's settings in the scope page. |
| `group` | string | no | Default group/tab for settings without their own `group`. |
| `order` | number | no | Display order among modules (ties broken by id, then declaration). |
| `docs_url` | string | no | Canonical upstream documentation; must be a whitespace-free `https://` URL. |
| `compatibility` | table | no | See below. |
| `support` | table | no | `maintainer`, `tested` (list), `level` (`supported`/`partial`/`read_only`). |
| `settings` | array | yes (≥1) | The setting definitions. |

Unknown fields are **rejected** (a typo fails loudly instead of being
ignored). Limits: 256 KiB per file, 512 settings per module.

## Compatibility (all optional, AND-ed)

| Field | Meaning |
|---|---|
| `desktops` | Desktop-environment names (case-insensitive: `GNOME`, `KDE`, `Hyprland`, `Xfce`, `COSMIC`, `Sway`). On an unidentified session the module is conservatively *not* enabled, with the uncertainty stated. |
| `distros` | Distribution ids/families matched against os-release `ID`/`ID_LIKE`. |
| `apps` | Desktop-file ids of the integrated applications (case-insensitive, `.desktop` suffix optional). |
| `required_backends` | Capability ids from discovery (`config:alacritty`, `package-manager:pacman`, `gsettings-schemas`, `dbus-session`) or kind aliases (`gsettings`, `dbus`). |
| `app_version` | `{ min = "…", max = "…" }`. Licoman cannot detect application versions yet, so a declared range is **conservatively treated as incompatible** with the uncertainty explained. |

Every failed dimension produces its **own preserved reason** — diagnostics
are data, not one opaque "incompatible".

## Field reference — settings

| Field | Type | Required | Meaning |
|---|---|---|---|
| `id` | string | yes | Module-relative stable id; the full identity is `<module-id>.<id>` and must be unique across the catalog. |
| `title` | string | yes | Row label (1–200). |
| `description` | string | no | Row explanation (≤1000). |
| `keywords` | list | no | Up to 16 extra search terms (≤64 chars each). The module title and app names are indexed automatically. |
| `type` | string | yes* | `boolean`, `enum`, `integer`, `float`, `string`, `path` or `list` (string list; read-only display). *Required for every control except `action`. |
| `control` | string | yes | `switch`, `checkbox`, `dropdown`, `number`, `slider`, `text`, `path_input`, `action`, `read_only`. Coherence with `type` is validated. |
| `policy` | string | no | `immediate`, `staged` (default) or `read_only`. `read_only` requires the `read_only` control so the UI never renders a control that cannot write. |
| `risk` | string | no | `low` (default), `medium`, `high`. High-risk rows carry a warning status line. |
| `required_auth` | string | no | Authorization boundary (e.g. a polkit action id); the setting renders permission-held. |
| `restart` | string | no | Named target that must restart for changes to take effect (rendered as a warning line). |
| `default` | typed | see rules | Declared default. **Required** for mock-backed boolean/enum/integer/float settings. Type-checked against `type`; range- and enum-checked. |
| `nullable` | bool | no | Only `string`/`path`; an empty value then means "not set" — never a literal `false`/`0`. |
| `min` / `max` | number | no | Inclusive numeric bounds (integer/float only). |
| `step` | number | no | Step for number/slider controls. |
| `unit` | string | no | Unit suffix (≤16 chars), e.g. `%`, `ms`, `K`. |
| `max_length` | integer | no | For string/path values (1–4096). |
| `options` | array | enum only | `{ value = "…", label = "…" }` entries (labels optional); values must be unique. |
| `group` | string | no | The Applications-window tab and search breadcrumb for this setting; falls back to the module `group`, then `category`. |
| `order` | number | no | Display order within the module (ties broken by declaration order). |
| `availability` | table | no | Per-setting predicate: `desktop`, `distro`, `app`, `backend` — all present fields must hold. |
| `backend` | table | yes | The data-only binding; see below. |

### Phase 06 write semantics (how `policy`/`risk`/`required_auth` are enforced)

The manifest's `policy` field is an **input**, never the decision. The core
derives the effective write policy: any `required_auth` ⇒ `privileged`
(refused by the deny-by-default broker — a declaration is not
authorization); a read-only control ⇒ `read_only`; `risk = "high"` ⇒ at
least `confirmation` (Apply opens a confirmation dialog showing effect and
target); otherwise the declared policy's staging applies. Every write —
apply, undo, reset — flows through the safe write engine
(`docs/architecture/write-engine.md`): conflict-checked against a re-read
of the source, backed up, verified by readback, and reported with honest
persistence. In this build the engine exists only for the fixture demos; a
live run never writes.

### Backend bindings (closed allowlist)

```toml
[settings.backend]
kind = "mock"                                    # in-process mock value source

[settings.backend]
kind = "gsettings"
schema = "org.gnome.desktop.interface"
key = "clock-show-weekday"                       # applicability checks both

[settings.backend]
kind = "dbus"
service = "org.freedesktop.portal.Desktop"
path = "/org/freedesktop/portal/desktop"
interface = "org.freedesktop.portal.Settings"
property = "color-scheme"                        # optional

[settings.backend]
kind = "file"
path = "alacritty/alacritty.toml"                # relative to $XDG_CONFIG_HOME
format = "toml"                                  # toml | json | keyfile
key = "font.size"                                # optional dotted key
resolution = "alacritty"                         # optional: alacritty | xdg_file
```

Applicability of a setting is checked against the **actual environment**:
a missing GSettings schema or key, or an absent bus service (session or
system), makes the setting unavailable — no value is faked.

The `resolution` field selects how a `file` binding locates and merges its
configuration (Phase 05; default `xdg_file` = one file under
`$XDG_CONFIG_HOME` read as-is). `alacritty` implements Alacritty's
documented search-path list plus the `general.import` merge
("the importing file being loaded last" — the last loader wins); see
`docs/architecture/backends.md` for the exact semantics and bounds.

## Registry lifecycle

```text
Load built-in manifests (embedded at compile time)
   -> Validate schema, unique ids, field coherence
   -> Query EnvironmentSnapshot and provider capabilities
   -> Compute applicability and compatibility reasons (per dimension)
   -> Index scopes, categories, groups, apps and search terms
   -> Bind typed value state (mock defaults immediately; real backends via
      the background read pass with origin/freshness provenance)
   -> Provide normalized data to the GPUI adapter
```

- Built-in manifests are **embedded at build time** (`include_str!`), so
  they are trusted at build/release time and no file can override them at
  runtime. Loading external data-only modules is deliberately deferred until
  a separate security review.
- Duplicate module ids reject the *later* definition (never last-one-wins);
  unrelated valid modules still load. Duplicate setting ids reject that
  module. Rejections are recorded with their diagnostics.
- On an environment refresh the registry recomputes applicability and the
  store reloads **only when the set of available settings changed**;
  navigation and staged edits survive otherwise.

## Tooling

```sh
licoman module validate <path>...   # read-only linter; prints located
                                    # diagnostics + applicability; exit 1 on
                                    # invalid manifests; never writes
licoman module schema               # JSON Schema from the canonical types
```

## Contributor workflow

1. Copy the closest file from `modules/examples/`.
2. Edit the manifest (validate with `licoman module validate`).
3. Add the file to the `MANIFESTS` table in `crates/core/src/modules/builtin.rs`.
4. Rebuild. No GPUI source changes — the UI derives all rows and controls
   from the registry.

**Deliberate limits:** no per-module custom layout/colors/fonts (the design
system owns presentation); no executable content of any kind; richer
validation rules (patterns, cross-field constraints) arrive with real
backends.
