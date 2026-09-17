#!/usr/bin/env bash
# Added by ke: build the release binary and install it on PATH as `ke`.
# The cargo bin target keeps its upstream name so upstream tests and tooling
# (CARGO_BIN_EXE_herdr, release scripts) keep working after merges.
#
# usage: scripts/ke-install.sh [--no-build]
#   --no-build        install the existing target/release/herdr without rebuilding
#   KE_INSTALL_DIR    install directory (default: ~/.local/bin)
set -euo pipefail

ROOT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
INSTALL_DIR=${KE_INSTALL_DIR:-"$HOME/.local/bin"}
TARGET_DIR=${CARGO_TARGET_DIR:-"$ROOT_DIR/target"}
BINARY="$TARGET_DIR/release/herdr"

build=1
for arg in "$@"; do
  case "$arg" in
    --no-build) build=0 ;;
    *)
      echo "usage: scripts/ke-install.sh [--no-build]" >&2
      exit 2
      ;;
  esac
done

if [[ "$build" == 1 ]]; then
  # build.rs needs the Zig version pinned by vendor/libghostty-vt/build.zig.zon (0.16.0 since
  # herdr 0.9.1). If it is neither on PATH nor given through ZIG, fall back to a toolchain
  # unpacked next to the repo (../ke-tools/zig-*/zig), preferring the pinned version when
  # several are unpacked. A local zig-macos27-shim wrapper wins when present: plain Zig
  # could not link on macOS 27 with 0.15.2.
  if [[ -z "${ZIG:-}" ]] && ! command -v zig >/dev/null 2>&1; then
    # Keep the path canonical: cargo fingerprints the ZIG value, so a different
    # spelling of the same path would rerun the zig build for nothing.
    tools_dir="$(dirname -- "$ROOT_DIR")/ke-tools"
    zig_version=$(sed -n 's/^\s*\.minimum_zig_version = "\(.*\)",$/\1/p' "$ROOT_DIR/vendor/libghostty-vt/build.zig.zon")
    for candidate in "$tools_dir"/zig-macos27-shim/zig "$tools_dir"/zig-*-"${zig_version:-none}"/zig "$tools_dir"/zig-*/zig; do
      if [[ -x "$candidate" ]]; then
        export ZIG="$candidate"
        break
      fi
    done
  fi

  cd "$ROOT_DIR"
  cargo build --release --locked
fi

if [[ ! -x "$BINARY" ]]; then
  echo "error: $BINARY not found; run without --no-build first" >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
# Copy to a temp name and rename so a running ke server keeps its old inode.
tmp="$INSTALL_DIR/.ke.$$"
trap 'rm -f "$tmp"' EXIT
install -m 755 "$BINARY" "$tmp"
mv -f "$tmp" "$INSTALL_DIR/ke"

echo "installed: $INSTALL_DIR/ke ($("$INSTALL_DIR/ke" --version))"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "note: $INSTALL_DIR is not on PATH" >&2 ;;
esac
