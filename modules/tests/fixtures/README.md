# Module test fixtures

The canonical, versioned fixture files live in the workspace-root
[`tests/fixtures/`](../../tests/fixtures/) directory (that location keeps
the crates' relative `include_str!`/path helpers stable and is what CI
exercises). This directory documents the mapping so module authors find
them:

| Fixture tree | Serves |
|---|---|
| `tests/fixtures/alacritty/` | Alacritty TOML-era configs + import chains, legacy YAML candidates, unknown-version roots (Phase 07 version gate) |
| `tests/fixtures/file-config/`, `file-home/` | Generic single-file TOML bindings and the mpv.conf grammar fixtures (Phase 08) |
| `tests/fixtures/discovery/` | Demo environment snapshots, compiled GSettings schema database, desktop entries |
| `tests/fixtures/gsettings/` | dconf user/system GVDB databases for origin/precedence tests |

Fixtures are inputs to the backend suites (`crates/core/src/read/*` tests
and the phase matrices). A new file-backed module should add a
representative config **plus** its malformed/absent/external-edit
counterparts here, referenced from the manifest's test notes.
