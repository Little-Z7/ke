#!/bin/sh
# Added by ke: installer for ke (壳), a modified fork of herdr.
#
#   curl -fsSL https://github.com/Little-Z7/ke/releases/latest/download/ke-install.sh | sh
#
# Environment:
#   KE_INSTALL_DIR    install directory (default: ~/.local/bin)
#   KE_RELEASE_TAG    install this release tag (e.g. ke-v0.1.0) instead of the latest release
#   KE_DOWNLOAD_BASE  download assets from this base URL instead of GitHub (mirrors, testing)
set -eu

BIN="ke"
REPO="Little-Z7/ke"
INSTALL_DIR="${KE_INSTALL_DIR:-$HOME/.local/bin}"

main() {
    echo ""
    echo "  ke (壳) installer — https://github.com/${REPO}"
    echo ""

    # detect platform
    OS="$(uname -s)"
    case "$OS" in
        Linux)  os="linux" ;;
        Darwin) os="macos" ;;
        *)      err "unsupported OS: $OS" ;;
    esac

    if [ "$OS" = "Linux" ] && [ "$(uname -o 2>/dev/null || true)" = "Android" ]; then
        err "Android/Termux is not supported by ke release binaries"
    fi

    ARCH="$(uname -m)"
    case "$ARCH" in
        x86_64|amd64)   arch="x86_64" ;;
        aarch64|arm64)  arch="aarch64" ;;
        *)              err "unsupported architecture: $ARCH" ;;
    esac

    log "detected ${os}/${arch}"

    need curl
    need awk

    ASSET="${BIN}-${os}-${arch}"
    if [ -n "${KE_DOWNLOAD_BASE:-}" ]; then
        BASE="${KE_DOWNLOAD_BASE%/}"
    elif [ -n "${KE_RELEASE_TAG:-}" ]; then
        BASE="https://github.com/${REPO}/releases/download/${KE_RELEASE_TAG}"
    else
        BASE="https://github.com/${REPO}/releases/latest/download"
    fi

    if command -v sha256sum >/dev/null 2>&1; then
        SHA256_TOOL="sha256sum"
    elif command -v shasum >/dev/null 2>&1; then
        SHA256_TOOL="shasum"
    elif command -v openssl >/dev/null 2>&1; then
        SHA256_TOOL="openssl"
    else
        err "SHA-256 verification requires sha256sum, shasum, or openssl"
    fi

    TMP="$(mktemp -d)"
    STAGED=""
    trap 'rm -rf "$TMP"; [ -z "$STAGED" ] || rm -f "$STAGED"' EXIT

    log "fetching checksums..."
    if ! curl -fsSL --retry 3 --connect-timeout 10 --max-time 20 "${BASE}/SHA256SUMS" -o "${TMP}/SHA256SUMS"; then
        err "can't fetch ${BASE}/SHA256SUMS"
    fi
    SHA256="$(awk -v asset="$ASSET" '$2 == asset || $2 == "*" asset { print tolower($1); exit }' "${TMP}/SHA256SUMS")"
    if [ -z "$SHA256" ]; then
        err "this release has no binary for ${os}/${arch}"
    fi
    if [ "${#SHA256}" -ne 64 ] || ! printf '%s\n' "$SHA256" | awk '/[^0-9a-f]/ { exit 1 }'; then
        err "release checksum for ${ASSET} is not a valid SHA-256"
    fi

    log "downloading ${ASSET}..."
    # Show a progress bar on a terminal so a slow link looks different from a hang, and stay quiet
    # when the output is piped or logged.
    if [ -t 2 ]; then
        PROGRESS="--progress-bar"
    else
        PROGRESS="--silent"
    fi
    # No --max-time here: the binary is ~26MB and a slow-but-working link is legitimate, so a fixed
    # deadline just fails honest downloads. Give up on a stall instead — under 4KB/s for 30s.
    # shellcheck disable=SC2086 # PROGRESS is one deliberate flag, not a path
    if ! curl -fSL $PROGRESS --retry 3 --connect-timeout 10 --speed-limit 4096 --speed-time 30 \
        "${BASE}/${ASSET}" -o "${TMP}/${BIN}"; then
        err "download failed from ${BASE}/${ASSET} — retry, or set KE_DOWNLOAD_BASE to a mirror"
    fi

    case "$SHA256_TOOL" in
        sha256sum) ACTUAL_SHA256="$(sha256sum < "${TMP}/${BIN}" | awk '{ print $1 }')" ;;
        shasum)    ACTUAL_SHA256="$(shasum -a 256 < "${TMP}/${BIN}" | awk '{ print $1 }')" ;;
        openssl)   ACTUAL_SHA256="$(openssl dgst -sha256 < "${TMP}/${BIN}" | awk '{ print $NF }')" ;;
    esac
    if [ "$ACTUAL_SHA256" != "$SHA256" ]; then
        err "downloaded ke checksum did not match"
    fi

    # install: stage next to the destination, then rename, so a running ke keeps its old file
    mkdir -p "$INSTALL_DIR"
    STAGED="${INSTALL_DIR}/.${BIN}.$$"
    cp "${TMP}/${BIN}" "$STAGED"
    chmod 755 "$STAGED"
    mv -f "$STAGED" "${INSTALL_DIR}/${BIN}"
    STAGED=""

    VERSION="$("${INSTALL_DIR}/${BIN}" --version 2>/dev/null || true)"
    log "installed ${INSTALL_DIR}/${BIN}${VERSION:+ — ${VERSION}}"

    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            echo ""
            warn "${INSTALL_DIR} is not in your PATH"
            echo "  add it to your shell config:"
            echo ""
            echo "    export PATH=\"${INSTALL_DIR}:\$PATH\""
            echo ""
            ;;
    esac

    echo ""
    log "run 'ke' to start. ke keeps its own config, socket and sessions (~/.config/ke) and does not touch an installed herdr."
    log "agent status hooks: ke reuses the ones installed by upstream herdr ('herdr integration install <agent>'); without them ke falls back to screen-based detection."
    log "to update, run this installer again."
    echo ""
}

log()  { printf '  \033[32m>\033[0m %s\n' "$1"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$1"; }
err()  { printf '  \033[31m✗\033[0m %s\n' "$1" >&2; exit 1; }

need() {
    if ! command -v "$1" >/dev/null 2>&1; then
        err "requires '$1' — install it first, or download a binary manually from https://github.com/${REPO}/releases"
    fi
}

main "$@"
