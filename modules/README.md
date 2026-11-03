# Modules

Modules are **data-only manifests** that tell Licoman where a setting lives
and how to present it. They contain no code and can never execute anything —
the manifest grammar (see [`schema/licoman-module.schema.json`](schema/
licoman-module.schema.json) and `docs/module-schema.md`) is a closed
allowlist of backend kinds, and validation rejects anything that looks like
an execution target.

## Directory layout

| Path | Purpose |
|---|---|
| `builtin/` | The shipped catalog. Embedded into the binary at compile time (`include_str!`, listed in `crates/core/src/modules/builtin.rs`), so they are trusted at build time and cannot be overridden by a same-named file. |
| `examples/` | Runnable example manifests: a mock application module, a GSettings module, a read-only service module, and a deliberately incompatible one (`fails-compatibility.toml`) that demonstrates preserved incompatibility reasons. |
| `schema/` | The canonical machine-validatable JSON Schema, dumped from `licoman module schema`. |
| `tests/fixtures/` | Points at the versioned fixture files (`tests/fixtures/`) used by the backend test suites. |

## How modules are loaded

1. **Built-ins (the only load path in this build).** `modules/builtin/*.toml`
   are embedded at compile time and validated again on every startup and by
   tests. A rejected built-in is logged and skipped — never half-loaded.
2. **User module directories are intentionally NOT loaded yet.** A
   declarative, user-controlled loading model
   (`$XDG_DATA_HOME/licoman/modules/`) is specified but requires the security
   review described in [`docs/security/module-trust.md`](../docs/security/
   module-trust.md). There is no dynamic loading, no plugins, and no remote
   manifest execution in this build.

## The catalog

Every module's support claims live in two places: the manifest itself
(`support` table) and the evidence-based compatibility matrix
([`docs/compatibility.md`](../docs/compatibility.md)). A module is only ever
as supported as its **backend** is on the machine in front of you — rows
that cannot be read or written here show honest unavailable states, never
sample values pretending to be real.

## Verifying a change

```sh
cargo run -p licoman -- module validate modules/builtin/   # linter (all 17 manifests)
cargo test --workspace                                     # registry + backend fixtures
```

The CI runs the manifest linter on every push; a module that fails
validation cannot reach a release build.
