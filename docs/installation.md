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
unset in `build()` because it breaks the wayland-backend link.

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

Build requirements: a stable Rust toolchain (edition 2024 — e.g. via
[rustup](https://rustup.rs)), `pkg-config`, and the gpui Linux build
dependencies:

```sh
# Debian / Ubuntu
sudo apt-get install -y \
  gcc g++ clang pkg-config \
  libfontconfig-dev \
  libwayland-dev \
  libwebkit2gtk-4.1-dev \
  libxkbcommon-x11-dev \
  libx11-xcb-dev \
  libssl-dev \
  libzstd-dev \
  libasound2-dev \
  libvulkan1 \
  libglib2.0-dev

# Arch
sudo pacman -S --needed \
  base-devel clang pkgconf \
  fontconfig wayland webkit2gtk-4.1 \
  libxkbcommon-x11 libx11 alsa-lib \
  openssl zstd vulkan-headers glib2
```

## First run

- The theme defaults to **Dark**; Light and System are selectable in the
  Preferences window and persist across restarts.
- Discovery runs at startup and stays passive: Licoman reads, it never
  starts or installs services.
- Every write is explicit: an Apply (and, for higher-risk rows, a
  confirmation) through the safe write engine — see
  [configuration-safety.md](configuration-safety.md).
