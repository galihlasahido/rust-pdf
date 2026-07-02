#!/usr/bin/env bash
#
# Downloads a prebuilt Pdfium shared library from bblanchon/pdfium-binaries
# (https://github.com/bblanchon/pdfium-binaries) for local development and
# testing of the `render` feature (src/render/).
#
# This is a *developer convenience* script, not something the published
# crate depends on at build time: `pdfium-render` loads the Pdfium shared
# library dynamically at *run time* (see src/render/mod.rs doc comments for
# the native-vs-FFI rationale), so there is no compile-time linkage here.
#
# The downloaded library is placed in `.pdfium/<platform>/` at the repo
# root, which is gitignored (see .gitignore: `.pdfium/`) because binary
# blobs should not live in git history. A real Tauri application ships this
# same binary as a bundled resource (see ARCHITECTURE.md, "Native vs FFI
# decision" section) using the platform's normal packaging mechanism, not
# by committing it to this library's repository.
#
# Usage:
#   scripts/fetch_pdfium.sh [platform]
#
# platform defaults to an autodetected value from `uname`. Valid values
# match the asset names published at:
#   https://github.com/bblanchon/pdfium-binaries/releases
# e.g. mac-arm64, mac-x64, linux-x64, linux-arm64, win-x64, win-arm64
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Pin a specific pdfium-binaries release so builds are reproducible; bump
# deliberately and re-verify rendering output when upgrading.
PDFIUM_RELEASE_TAG="${PDFIUM_RELEASE_TAG:-chromium/7920}"

detect_platform() {
    local os arch
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os" in
        Darwin)
            case "$arch" in
                arm64) echo "mac-arm64" ;;
                x86_64) echo "mac-x64" ;;
                *) echo "unsupported-$os-$arch" ;;
            esac
            ;;
        Linux)
            case "$arch" in
                aarch64) echo "linux-arm64" ;;
                x86_64) echo "linux-x64" ;;
                *) echo "unsupported-$os-$arch" ;;
            esac
            ;;
        *)
            echo "unsupported-$os-$arch"
            ;;
    esac
}

PLATFORM="${1:-$(detect_platform)}"

if [[ "$PLATFORM" == unsupported-* ]]; then
    echo "error: could not autodetect a supported platform (got: $PLATFORM)" >&2
    echo "pass one explicitly, e.g.: scripts/fetch_pdfium.sh mac-arm64" >&2
    exit 1
fi

DEST_DIR="$REPO_ROOT/.pdfium/$PLATFORM"
ASSET="pdfium-$PLATFORM.tgz"
URL="https://github.com/bblanchon/pdfium-binaries/releases/download/${PDFIUM_RELEASE_TAG}/${ASSET}"

if [[ -d "$DEST_DIR/lib" ]] && [[ -n "$(find "$DEST_DIR/lib" -maxdepth 1 \( -name '*.dylib' -o -name '*.so' -o -name '*.dll' \) -print -quit 2>/dev/null)" ]]; then
    echo "pdfium already present at $DEST_DIR/lib, skipping download (delete the directory to force re-fetch)"
    exit 0
fi

mkdir -p "$DEST_DIR"
TMP_ARCHIVE="$(mktemp -t pdfium-download.XXXXXX).tgz"
trap 'rm -f "$TMP_ARCHIVE"' EXIT

echo "downloading $URL"
curl -fL --retry 3 -o "$TMP_ARCHIVE" "$URL"
tar -xzf "$TMP_ARCHIVE" -C "$DEST_DIR"

echo "pdfium $PDFIUM_RELEASE_TAG for $PLATFORM installed at $DEST_DIR"
echo "set RUST_PDF_PDFIUM_LIB_DIR=$DEST_DIR/lib to point the render feature at it, e.g.:"
echo "  RUST_PDF_PDFIUM_LIB_DIR=$DEST_DIR/lib cargo test --features render"
