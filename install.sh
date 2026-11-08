#!/usr/bin/env bash
# Licoman — install from source.
#
# Builds the release binary with cargo and installs it alongside the
# launcher, hicolor icons, AppStream metadata, license notices and offline
# documentation — exactly the file set the Arch package
# (packaging/licoman/PKGBUILD) ships, for systems without pacman.
#
# Usage:
#   ./install.sh                 # system install to /usr/local (sudo only if needed)
#   ./install.sh --user          # user install to ~/.local (no root, no sudo)
#   ./install.sh --prefix DIR    # custom prefix
#   ./install.sh --no-build      # (re)install without rebuilding
#   ./install.sh --uninstall     # remove a previous install for the chosen prefix
#
# Uninstalling removes only the files this script installs — your Licoman
# preferences and any backups it made stay in $XDG_CONFIG_HOME/licoman and
# $XDG_DATA_HOME/licoman unless you delete them yourself.
#
# Icon lookup note: `Icon=io.github.barscn.Licoman` resolves for every
# supported prefix without edits, because the default XDG_DATA_DIRS
# (/usr/local/share:/usr/share) plus ~/.local/share all sit on the icon
# search path; only the launcher's Exec line is rewritten per prefix.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APPID=io.github.barscn.Licoman
# Same size set the PKGBUILD ships (assets/icons/hicolor layout).
ICON_SIZES=(16 20 22 24 32 36 48 64 72 96 128 192 256 512 1024)
DOCS=(installation.md usage.md configuration-safety.md compatibility.md privacy.md)

PREFIX=/usr/local
UNINSTALL=0
BUILD=1

usage() {
  # Print the header comment (everything after the shebang up to the first
  # non-comment line), so --help can never drift from the file's own docs.
  awk 'NR==1 {next} /^#/ {sub(/^# ?/, ""); print; next} {exit}' "${BASH_SOURCE[0]}"
}

die() { echo "install.sh: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --user) PREFIX="$HOME/.local"; shift ;;
    --prefix) [ -n "${2:-}" ] || die "--prefix needs a directory"; PREFIX="$2"; shift 2 ;;
    --prefix=*) PREFIX="${1#*=}"; shift ;;
    --uninstall) UNINSTALL=1; shift ;;
    --no-build) BUILD=0; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
done

# Root only when the prefix is not writable by the invoking user. Once a
# sudo mkdir happens, every file operation below goes through SUDO too, so
# a mixed user/root install can never occur.
SUDO=""
ensure_dirs() {
  if ! mkdir -p "$@" 2>/dev/null; then
    command -v sudo >/dev/null 2>&1 \
      || die "cannot write to $PREFIX and sudo is unavailable"
    SUDO=sudo
    mkdir -p "$@"
  fi
}
as_root() { if [ -n "$SUDO" ]; then "$SUDO" "$@"; else "$@"; fi }

BIN="$REPO_ROOT/target/release/licoman"

if [ "$UNINSTALL" -eq 1 ]; then
  echo "Removing Licoman from $PREFIX ..."
  ensure_dirs "$PREFIX/bin" "$PREFIX/share/doc/licoman"
  as_root rm -f \
    "$PREFIX/bin/licoman" \
    "$PREFIX/share/applications/$APPID.desktop" \
    "$PREFIX/share/metainfo/$APPID.metainfo.xml" \
    "$PREFIX/share/icons/hicolor/scalable/apps/$APPID.svg" \
    "$PREFIX/share/licenses/licoman/LICENSE" \
    "$PREFIX/share/licenses/licoman/NOTICES"
  for _size in "${ICON_SIZES[@]}"; do
    as_root rm -f "$PREFIX/share/icons/hicolor/${_size}x${_size}/apps/$APPID.png"
  done
  as_root rm -rf "$PREFIX/share/doc/licoman"
  echo "Done. Preferences and backups in \$XDG_CONFIG_HOME/licoman and"
  echo "\$XDG_DATA_HOME/licoman were left in place."
  exit 0
fi

if [ "$BUILD" -eq 1 ]; then
  command -v cargo >/dev/null 2>&1 \
    || die "cargo not found — install a Rust toolchain (https://rustup.rs)"
  # Same command as the repository's release gate and the PKGBUILD build():
  # plain release profile, --locked so the vendored lock file is authoritative.
  (cd "$REPO_ROOT" && cargo build --release --locked -p licoman)
fi
[ -x "$BIN" ] || die "no binary at $BIN — build first (drop --no-build)"

echo "Installing Licoman to $PREFIX ..."
ensure_dirs \
  "$PREFIX/bin" \
  "$PREFIX/share/applications" \
  "$PREFIX/share/metainfo" \
  "$PREFIX/share/icons/hicolor/scalable/apps" \
  "$PREFIX/share/licenses/licoman" \
  "$PREFIX/share/doc/licoman"

as_root install -Dm755 "$BIN" "$PREFIX/bin/licoman"

# The launcher ships with Exec=/usr/bin/licoman (the package layout); a
# source install must point it at the prefix it actually landed in.
_launcher="$(mktemp)"
trap 'rm -f "$_launcher"' EXIT
sed "s|^Exec=.*|Exec=$PREFIX/bin/licoman|" \
  "$REPO_ROOT/packaging/desktop/$APPID.desktop" > "$_launcher"
as_root install -Dm644 "$_launcher" "$PREFIX/share/applications/$APPID.desktop"

as_root install -Dm644 "$REPO_ROOT/packaging/metainfo/$APPID.metainfo.xml" \
  "$PREFIX/share/metainfo/$APPID.metainfo.xml"

_icon="$REPO_ROOT/assets/icons/hicolor"
as_root install -Dm644 "$_icon/scalable/apps/$APPID.svg" \
  "$PREFIX/share/icons/hicolor/scalable/apps/$APPID.svg"
for _size in "${ICON_SIZES[@]}"; do
  as_root install -Dm644 "$_icon/${_size}x${_size}/apps/$APPID.png" \
    "$PREFIX/share/icons/hicolor/${_size}x${_size}/apps/$APPID.png"
done

as_root install -Dm644 "$REPO_ROOT/LICENSE" "$PREFIX/share/licenses/licoman/LICENSE"
as_root install -Dm644 "$REPO_ROOT/NOTICES" "$PREFIX/share/licenses/licoman/NOTICES"
for _doc in "${DOCS[@]}"; do
  as_root install -m644 "$REPO_ROOT/docs/$_doc" "$PREFIX/share/doc/licoman/$_doc"
done

# Best-effort cache refreshes; both desktops rebuild these lazily anyway.
as_root update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
as_root gtk-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" 2>/dev/null || true

echo "Installed:"
echo "  $PREFIX/bin/licoman"
echo "  $PREFIX/share/applications/$APPID.desktop"
echo "  $PREFIX/share/metainfo/$APPID.metainfo.xml"
echo "  $PREFIX/share/icons/hicolor/... ($APPID)"
echo "Run 'licoman' from your application menu or a terminal."
