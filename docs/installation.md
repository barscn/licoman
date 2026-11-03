# Installing Licoman

Licoman is a native desktop application (Rust + GPUI). It has no runtime
web stack, no daemon and no telemetry. Requirements:

- A Wayland or X11 session with a working Vulkan user-space stack
  (loader + a GPU driver manifest — see
  [troubleshooting.md](troubleshooting.md) if startup says otherwise)
- glib/gio (present on every desktop Linux system; the native GSettings
  backend links it)
- x86_64 (aarch64 is not packaged yet)

## Packages

### Arch (PKGBUILD — the tested target)

The repository ships `packaging/licoman/PKGBUILD`. From a release tarball
(`git archive` of the release tag):

```sh
makepkg -ef
sudo pacman -U licoman-0.1.0-1-x86_64.pkg.tar.zst
```

The package installs `/usr/bin/licoman`, the launcher
(`io.github.barscn.Licoman.desktop`), hicolor icons, AppStream metadata,
license notices and offline documentation. Uninstall
(`sudo pacman -R licoman`) removes only those package-owned files — your
Licoman preferences and any backups it made stay in
`$XDG_CONFIG_HOME/licoman` and `$XDG_DATA_HOME/licoman` unless you delete
them yourself.

Note: the PKGBUILD builds with the same plain `cargo build --release` the
repository's release gate uses; the distro-wide LTO/RUSTFLAGS set is
unset in `build()` because it breaks the wayland-backend link (documented
in the Phase 09 report).

Other distributions: no .deb or .rpm exists yet. Placeholder packages are
deliberately not shipped — a package lands only with an actually tested
target (the same honesty rule as everything else in Licoman).

## Running from source

```sh
git clone https://github.com/barscn/licoman
cd licoman
cargo run --release -p licoman        # the GUI application
cargo run -p licoman -- --diagnostics # headless environment facts (read-only)
```

Build requirements: a stable Rust toolchain (edition 2024), plus the usual
gpui Linux build dependencies (clang, fontconfig, wayland, libxkbcommon,
alsa, vulkan headers — see `.github/workflows/ci.yml` for the exact list
CI uses).

## First run

- The theme defaults to **Dark**; Light and System are selectable in the
  Preferences window and persist across restarts.
- Discovery runs at startup and stays passive: Licoman reads, it never
  starts or installs services.
- Every write is explicit: an Apply (and, for higher-risk rows, a
  confirmation) through the safe write engine — see
  [configuration-safety.md](configuration-safety.md).
