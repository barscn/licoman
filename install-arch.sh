#!/usr/bin/env bash
# Licoman — build and install the Arch Linux package in one step.
#
# Wraps packaging/licoman/PKGBUILD (the tested packaging target): produces
# the source tarball the PKGBUILD expects, pins its sha256 into a staging
# copy of the PKGBUILD, runs makepkg and installs the resulting package
# with pacman. The repository tree is never modified — all makepkg
# artifacts land in packaging/licoman/build/, and the built package file
# is copied back to packaging/licoman/ so it survives the cleanup.
#
# Usage:
#   ./install-arch.sh                # tarball from tag v<pkgver>, else current HEAD
#   ./install-arch.sh --worktree     # archive the working tree incl. uncommitted
#                                    # changes (tracked + non-ignored untracked files)
#   ./install-arch.sh --tag v0.1.0   # archive an explicit tag or commit
#   ./install-arch.sh --no-install   # build the package, skip the pacman -U step
#   ./install-arch.sh --no-check     # skip the PKGBUILD check() test suite
#   ./install-arch.sh --install-deps # install the build dependencies first
#   ./install-arch.sh --keep         # keep packaging/licoman/build/ for inspection
#
# The staging build is a cold cargo release build (the PKGBUILD pins its
# own CARGO_TARGET_DIR and CARGO_HOME), so the first run takes a while.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PKG_DIR="$REPO_ROOT/packaging/licoman"
BUILD_DIR="$PKG_DIR/build"
PKGBUILD="$PKG_DIR/PKGBUILD"

MODE=ref   # ref | worktree
REF=""
INSTALL=1
RUN_CHECK=1
KEEP=0

die() { echo "install-arch.sh: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --worktree) MODE=worktree; shift ;;
    --tag) [ -n "${2:-}" ] || die "--tag needs a tag or commit"; MODE=ref; REF="$2"; shift 2 ;;
    --tag=*) MODE=ref; REF="${1#*=}"; shift ;;
    --no-install) INSTALL=0; shift ;;
    --no-check) RUN_CHECK=0; shift ;;
    --install-deps) INSTALL_DEPS=1; shift ;;
    --keep) KEEP=1; shift ;;
    -h|--help) awk 'NR==1 {next} /^#/ {sub(/^# ?/, ""); print; next} {exit}' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
done

command -v pacman >/dev/null 2>&1 && command -v makepkg >/dev/null 2>&1 \
  || die "pacman/makepkg not found — this script is for Arch Linux; use ./install.sh elsewhere"

if [ -n "${INSTALL_DEPS:-}" ]; then
  sudo pacman -S --needed base-devel clang
fi

# Build-side prerequisites, checked as commands (not package names): a
# rustup-managed cargo satisfies the build exactly like pacman's rust
# package does. The hint maps each missing tool to the repo package that
# ships it — the repos have no package named "cargo"; rust ships it, and
# rust is only suggested when no cargo exists (rust and rustup conflict,
# so it must never be pushed onto a rustup machine).
_missing=()
for _tool in makepkg cargo clang pkg-config; do
  command -v "$_tool" >/dev/null 2>&1 || _missing+=("$_tool")
done
if [ "${#_missing[@]}" -gt 0 ]; then
  _pkgs=(base-devel)   # makepkg and pkg-config ship in base-devel
  command -v clang >/dev/null 2>&1 || _pkgs+=(clang)
  command -v cargo >/dev/null 2>&1 || _pkgs+=(rust)
  echo "install-arch.sh: missing build tools: ${_missing[*]}" >&2
  echo "install them with:  sudo pacman -S --needed ${_pkgs[*]}" >&2
  exit 1
fi

pkgname="$(sed -n 's/^pkgname=//p' "$PKGBUILD")"
pkgver="$(sed -n 's/^pkgver=//p' "$PKGBUILD")"
[ -n "$pkgname" ] && [ -n "$pkgver" ] || die "cannot read pkgname/pkgver from $PKGBUILD"

# Staging starts fresh every run: stale artifacts from an older pkgver would
# otherwise be picked up by makepkg's globs.
rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR"

tarprefix="$pkgname-$pkgver"
tarball="$BUILD_DIR/$tarprefix.tar.gz"

if [ "$MODE" = worktree ]; then
  echo "Packing the working tree (tracked + non-ignored untracked files) ..."
  # build/ is git-ignored, so --exclude-standard keeps the staging directory
  # out of its own tarball.
  (cd "$REPO_ROOT" && git ls-files -co --exclude-standard -z \
    | tar --null -T - --transform "s,^,$tarprefix/," -czf "$tarball")
else
  if [ -z "$REF" ]; then
    if git -C "$REPO_ROOT" rev-parse -q --verify "refs/tags/v$pkgver" >/dev/null 2>&1; then
      REF="v$pkgver"
    else
      REF=HEAD
      echo "note: no v$pkgver tag exists — archiving the committed state of HEAD"
    fi
  fi
  if [ "$REF" = HEAD ] && [ -n "$(git -C "$REPO_ROOT" status --porcelain)" ]; then
    echo "note: uncommitted changes are NOT in the package (use --worktree to include them)"
  fi
  echo "Archiving $REF -> ${tarball#$REPO_ROOT/}"
  git -C "$REPO_ROOT" archive --format=tar.gz --prefix="$tarprefix/" -o "$tarball" "$REF"
fi

# The repo PKGBUILD keeps sha256sums=('SKIP') until the release process fills
# it in; the staging copy is pinned to the tarball we just produced, so
# makepkg verifies exactly what it installs. A real checksum never ships as
# SKIP, and the repository copy stays untouched.
sum="$(sha256sum "$tarball")"; sum="${sum%% *}"
sed "s|^\s*sha256sums=.*|sha256sums=('$sum')|" "$PKGBUILD" > "$BUILD_DIR/PKGBUILD"

makepkg_args=(-f)
[ "$RUN_CHECK" -eq 1 ] || makepkg_args+=(--nocheck)

echo "Building the package (cold release build — this takes a while) ..."
(cd "$BUILD_DIR" && makepkg "${makepkg_args[@]}")

pkgfile="$(find "$BUILD_DIR" -maxdepth 1 -name "$pkgname-$pkgver-*.pkg.tar.zst" -print -quit)"
[ -n "$pkgfile" ] || die "makepkg finished but no $pkgname-$pkgver-*.pkg.tar.zst was produced"

# Keep the artifact outside the staging dir so it survives --keep-less cleanup.
cp "$pkgfile" "$PKG_DIR/"
kept="${PKG_DIR}/$(basename "$pkgfile")"
echo "Package: $kept"

if [ "$INSTALL" -eq 1 ]; then
  echo "Installing (pacman resolves the runtime dependencies) ..."
  sudo pacman -U "$kept"
else
  echo "Not installing (--no-install). Install later with:"
  echo "  sudo pacman -U \"$kept\""
fi

if [ "$KEEP" -eq 1 ]; then
  echo "Staging kept in ${BUILD_DIR#$REPO_ROOT/}"
else
  rm -rf "$BUILD_DIR"
fi
