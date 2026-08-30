#!/usr/bin/env bash
# setup-cloud-sql-proxy.sh — install Google's Cloud SQL Auth Proxy, which frms'
# Database tool starts for GCP Cloud SQL connections.
#
# The proxy is a single static Go binary published to a Google bucket, not a
# distro package, so an .rpm/.deb install of frms can't pull it in. This
# one-shot helper does the whole setup:
#   1. works out the right build for this OS/arch,
#   2. resolves the latest version and its published SHA-256,
#   3. downloads it, verifies the checksum, and installs it onto PATH.
#
# It ships in the package as `frms-setup-cloud-sql-proxy` and is also invoked by
# install.sh and by the Install button in frms' Cloud SQL section, so this file
# is the single source of truth for the flow — mirroring scripts/setup-claude.sh.
#
# Re-running it is safe: an existing install is detected and skipped unless
# --force is given.
#
# Usage: setup-cloud-sql-proxy.sh [--force] [--dir DIR] [--version vX.Y.Z]
#   --dir      install location (default: /usr/local/bin, or ~/.local/bin
#              without root). Also settable as FRMS_CSP_DIR.
#   --version  pin a release instead of taking the latest.

set -euo pipefail

REPO="GoogleCloudPlatform/cloud-sql-proxy"
BUCKET="https://storage.googleapis.com/cloud-sql-connectors/cloud-sql-proxy"
API="https://api.github.com/repos/$REPO/releases/latest"

# Used only when the GitHub API can't be reached. Bump alongside a release when
# convenient; it exists so an offline-ish machine still gets a working proxy.
FALLBACK_VERSION="v2.25.4"

FORCE=0
DIR="${FRMS_CSP_DIR:-}"
VERSION=""

while [ $# -gt 0 ]; do
    case "$1" in
        --force)   FORCE=1; shift ;;
        --dir)     DIR="${2:?--dir needs a directory}"; shift 2 ;;
        --version) VERSION="${2:?--version needs a tag}"; shift 2 ;;
        -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
        *)         echo "Unknown option: $1" >&2; exit 2 ;;
    esac
done

echo "── Cloud SQL Auth Proxy setup ───────────────────────────────────────"

if [ "$FORCE" -eq 0 ] && command -v cloud-sql-proxy &>/dev/null; then
    echo "Cloud SQL Proxy is already installed ($(command -v cloud-sql-proxy)). ✓"
    cloud-sql-proxy --version 2>/dev/null || true
    echo
    echo "Re-run with --force to replace it."
    exit 0
fi

# ── which build ───────────────────────────────────────────────────────────────
# Google's asset names: cloud-sql-proxy.<os>.<arch>, and .x64.exe / .arm64.exe
# on Windows.
case "$(uname -s)" in
    Linux)  OS=linux  ;;
    Darwin) OS=darwin ;;
    *)      echo "Unsupported OS: $(uname -s). Install manually from" >&2
            echo "  https://cloud.google.com/sql/docs/mysql/sql-proxy" >&2
            exit 1 ;;
esac
case "$(uname -m)" in
    x86_64|amd64)  ARCH=amd64 ;;
    aarch64|arm64) ARCH=arm64 ;;
    i386|i686)     ARCH=386   ;;
    *)             echo "Unsupported architecture: $(uname -m)." >&2; exit 1 ;;
esac
ASSET="cloud-sql-proxy.$OS.$ARCH"

command -v curl &>/dev/null || { echo "curl is required but not installed." >&2; exit 1; }

# ── version + published checksum ──────────────────────────────────────────────
# The release body carries a markdown table of "filename | sha256", which is the
# only place Google publishes the hashes — the bucket has no .sha256 siblings.
# So one API call gives us both the tag to download and the hash to verify it.
SHA=""
if RELEASE="$(curl -fsSL --max-time 30 "$API" 2>/dev/null)"; then
    [ -n "$VERSION" ] || VERSION="$(printf '%s' "$RELEASE" |
        grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 |
        sed 's/.*"\([^"]*\)"$/\1/')"
    # Escape the dots so the asset name can't match a neighbouring row.
    ASSET_RE="$(printf '%s' "$ASSET" | sed 's/\./\\./g')"
    SHA="$(printf '%s' "$RELEASE" |
        grep -o "$ASSET_RE)[^0-9a-f]*[0-9a-f]\{64\}" | head -1 |
        grep -o '[0-9a-f]\{64\}' || true)"
fi

if [ -z "$VERSION" ]; then
    VERSION="$FALLBACK_VERSION"
    echo "⚠ Couldn't reach the GitHub release API — falling back to $VERSION."
fi

URL="$BUCKET/$VERSION/$ASSET"
echo "Version:  $VERSION"
echo "Build:    $ASSET"

# ── where it goes ─────────────────────────────────────────────────────────────
# /usr/local/bin when we can write there (with or without sudo), so every shell
# and a desktop-launched frms find it on PATH; otherwise the user's own bin.

# Whether we could create/write $1 as this user. A directory that doesn't exist
# yet is judged by its nearest existing ancestor, so `--dir ~/somewhere/new`
# doesn't get mistaken for a location needing root.
can_write() {
    local d="$1"
    while [ ! -e "$d" ]; do d="$(dirname "$d")"; done
    [ -w "$d" ]
}

SUDO=""
if [ -z "$DIR" ]; then
    if can_write /usr/local/bin; then
        DIR=/usr/local/bin
    elif command -v sudo &>/dev/null; then
        DIR=/usr/local/bin
        SUDO=sudo
    else
        DIR="$HOME/.local/bin"
    fi
elif ! can_write "$DIR" && command -v sudo &>/dev/null; then
    SUDO=sudo
fi
$SUDO mkdir -p "$DIR"
TARGET="$DIR/cloud-sql-proxy"
echo "Install:  $TARGET"
echo

# ── download, verify, install ─────────────────────────────────────────────────
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Downloading $URL"
curl -fL --progress-bar --max-time 600 -o "$TMP/cloud-sql-proxy" "$URL" || {
    echo "Download failed. Check the version tag and your network." >&2
    exit 1
}

if [ -n "$SHA" ]; then
    if   command -v sha256sum &>/dev/null; then got="$(sha256sum   "$TMP/cloud-sql-proxy" | cut -d' ' -f1)"
    elif command -v shasum    &>/dev/null; then got="$(shasum -a 256 "$TMP/cloud-sql-proxy" | cut -d' ' -f1)"
    else got=""; echo "⚠ No sha256sum/shasum available — skipping checksum verification."
    fi
    if [ -n "$got" ]; then
        if [ "$got" != "$SHA" ]; then
            echo "✗ Checksum mismatch — refusing to install." >&2
            echo "  expected $SHA" >&2
            echo "  got      $got" >&2
            exit 1
        fi
        echo "Checksum verified ($SHA). ✓"
    fi
else
    echo "⚠ No published checksum found for $ASSET — installing unverified."
fi

chmod 755 "$TMP/cloud-sql-proxy"
$SUDO install -m 755 "$TMP/cloud-sql-proxy" "$TARGET"

echo
echo "Installed: $TARGET ✓"
"$TARGET" --version || true

# ~/.local/bin is standard but not on every distro's default PATH, and a
# desktop-launched frms inherits the session environment rather than a shell's.
# frms probes this location directly, so the app finds it either way — the
# warning is for the user's own shells.
case ":$PATH:" in
    *":$DIR:"*) ;;
    *) echo
       echo "⚠ $DIR is not on your PATH — add it to use 'cloud-sql-proxy' in a shell:"
       echo "    export PATH=\"$DIR:\$PATH\"" ;;
esac

echo
echo "Done. frms' Database tool will find the proxy automatically."
echo "In the Database tab: Provider → GCP Cloud SQL, enter the instance"
echo "connection name (project:region:instance) and your credentials file,"
echo "then press Start (or just Connect)."
