#!/usr/bin/env bash
# install.sh — install frms as a desktop application (user-local).
#
# Fedora Silverblue friendly: everything lands under ~/.local, so no
# rpm-ostree layering and no reboot. After install, frms appears in the
# app grid and can be pinned to the dash/taskbar.
#
#   ./install.sh               check deps, build (release), install
#   ./install.sh --check-deps  only run the dependency check/install, no build
#   ./install.sh --skip-deps   build + install without touching dependencies
#   ./install.sh --uninstall   remove the installed files

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

BIN_DIR="$HOME/.local/bin"
APP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/256x256/apps"

BIN="$BIN_DIR/frms"
DESKTOP="$APP_DIR/frms.desktop"
ICON="$ICON_DIR/frms.png"

# ─────────────────────────────────────────────────────────────────────────────
# Dependency table
# ─────────────────────────────────────────────────────────────────────────────
# Each row: cmd|apt|dnf|pacman|zypper|brew|tier|description
#   cmd   the command frms invokes at runtime (probed with `command -v`)
#   tier  core      → frms is broken/severely degraded without it
#         optional  → a single feature stops working without it
# A package field of "-" means that manager has no package for it (skip).
read -r -d '' DEPS_TABLE <<'ROWS' || true
curl|curl|curl|curl|curl|curl|core|HTTP networking (chat / research panes, usage stats)
chromium|chromium|chromium|chromium|chromium|chromium|optional|in-app web preview & page screenshots
ffmpeg|ffmpeg|ffmpeg|ffmpeg|ffmpeg|ffmpeg|optional|splash-video decoding (cosmetic)
yt-dlp|yt-dlp|yt-dlp|yt-dlp|yt-dlp|yt-dlp|optional|splash-video download (cosmetic)
sshpass|sshpass|sshpass|sshpass|sshpass|sshpass|optional|scp file transfer with password auth
ssh|openssh-client|openssh-clients|openssh|openssh|-|optional|remote receivers over ssh
ROWS

detect_pkg_manager() {
    if   command -v apt-get    &>/dev/null; then echo apt
    elif command -v dnf        &>/dev/null; then echo dnf
    elif command -v rpm-ostree &>/dev/null; then echo rpm-ostree
    elif command -v pacman  &>/dev/null; then echo pacman
    elif command -v zypper  &>/dev/null; then echo zypper
    elif command -v brew    &>/dev/null; then echo brew
    else echo ""; fi
}

# Column index into a DEPS_TABLE row for the given package manager.
pkg_col_for() {
    case "$1" in
        apt) echo 2;; dnf|rpm-ostree) echo 3;; pacman) echo 4;; zypper) echo 5;; brew) echo 6;;
        *) echo 0;;
    esac
}

# Run the manager's install command for the given package list (batched).
run_install() {
    local mgr="$1"; shift
    [ "$#" -eq 0 ] && return 0
    echo "  → installing: $*"
    case "$mgr" in
        apt)    sudo apt-get update -qq && sudo apt-get install -y "$@" ;;
        dnf)    sudo dnf install -y "$@" ;;
        rpm-ostree)
            # Immutable host (Silverblue/Kinoita): layering needs a reboot to take effect.
            sudo rpm-ostree install --idempotent --allow-inactive "$@"
            echo "  ⚠ rpm-ostree layered the packages — reboot to activate them before running frms."
            ;;
        pacman) sudo pacman -S --needed --noconfirm "$@" ;;
        zypper) sudo zypper --non-interactive install "$@" ;;
        brew)   brew install "$@" ;;
    esac
}

# Pick the clipboard helper appropriate for the active display server.
# Wayland → wl-clipboard (wl-copy/wl-paste); X11 (e.g. Mint Cinnamon) → xclip.
clipboard_dep() {
    local mgr="$1"
    if [ -n "${WAYLAND_DISPLAY:-}" ]; then
        case "$mgr" in
            apt|dnf|rpm-ostree|pacman|zypper) echo "wl-copy wl-clipboard" ;;
            brew) echo "" ;;            # macOS uses pbcopy; not relevant here
        esac
    else
        case "$mgr" in
            apt|dnf|rpm-ostree|pacman|zypper) echo "xclip xclip" ;;
            brew) echo "" ;;
        esac
    fi
}

check_deps() {
    local mgr; mgr="$(detect_pkg_manager)"
    local col;  col="$(pkg_col_for "$mgr")"

    echo "── Dependency check ─────────────────────────────────────────────────"
    if [ -z "$mgr" ]; then
        echo "No supported package manager found (apt/dnf/pacman/zypper/brew)."
        echo "Install the following manually if any are missing: curl, xclip/wl-clipboard."
        return 0
    fi
    echo "Package manager: $mgr"

    local -a missing_core=() missing_opt=()
    local -a label_core=()   label_opt=()

    # Clipboard (display-server dependent) is checked first, separately.
    local clip; clip="$(clipboard_dep "$mgr")"
    if [ -n "$clip" ]; then
        local clip_cmd clip_pkg
        clip_cmd="${clip%% *}"; clip_pkg="${clip##* }"
        if ! command -v "$clip_cmd" &>/dev/null; then
            missing_core+=("$clip_pkg")
            label_core+=("$clip_cmd — clipboard copy/paste")
        fi
    fi

    # Table-driven deps.
    while IFS='|' read -r cmd apt dnf pac zyp brew tier desc; do
        [ -z "$cmd" ] && continue
        command -v "$cmd" &>/dev/null && continue
        local pkg
        pkg="$(echo "$cmd|$apt|$dnf|$pac|$zyp|$brew|$tier|$desc" | cut -d'|' -f"$col")"
        [ "$pkg" = "-" ] && continue   # no package for this manager
        if [ "$tier" = core ]; then
            missing_core+=("$pkg"); label_core+=("$cmd — $desc")
        else
            missing_opt+=("$pkg");  label_opt+=("$cmd — $desc")
        fi
    done <<< "$DEPS_TABLE"

    if [ "${#missing_core[@]}" -eq 0 ] && [ "${#missing_opt[@]}" -eq 0 ]; then
        echo "All runtime dependencies present. ✓"
    fi

    if [ "${#missing_core[@]}" -gt 0 ]; then
        echo
        echo "Missing REQUIRED dependencies:"
        printf '  • %s\n' "${label_core[@]}"
        read -r -p "Install these via $mgr now? [Y/n] " ans
        case "${ans:-Y}" in
            [nN]*) echo "  ⚠ Skipped — frms will not work correctly without them." ;;
            *)     run_install "$mgr" "${missing_core[@]}" ;;
        esac
    fi

    if [ "${#missing_opt[@]}" -gt 0 ]; then
        echo
        echo "Missing OPTIONAL dependencies (features that need them):"
        printf '  • %s\n' "${label_opt[@]}"
        read -r -p "Install these too? [y/N] " ans
        case "${ans:-N}" in
            [yY]*) run_install "$mgr" "${missing_opt[@]}" ;;
            *)     echo "  Skipped — those features stay disabled until installed." ;;
        esac
    fi

    # Claude Code CLI is npm-distributed, not a distro package, and Build/agent
    # panes drive it. Delegate to the bundled setup helper (the single source of
    # truth, also shipped as `frms-setup-claude`): it installs Node.js/npm if
    # missing, then Claude Code, then offers the one-time sign-in.
    if ! command -v claude &>/dev/null; then
        echo
        echo "  'claude' (Claude Code CLI) is not installed — Build/agent panes need it."
        echo "  This installs Node.js/npm (if missing), then Claude Code."
        read -r -p "  Set up Claude Code now? [Y/n] " ans
        case "${ans:-Y}" in
            [nN]*) echo "  Skipped — Build panes will show install instructions until it's present." ;;
            *)     "$SCRIPT_DIR/scripts/setup-claude.sh" || \
                       echo "  ⚠ Claude Code setup didn't complete — see the message above." ;;
        esac
    fi

    # Research/Chat panes use the Anthropic API directly, not the CLI: enter an
    # API key in frms' Profile tab (get one at console.anthropic.com).
    echo
}

# Build-from-source prerequisites (Rust toolchain + a C linker).
check_build_deps() {
    local missing=0
    if ! command -v cargo &>/dev/null; then
        echo "Rust toolchain (cargo) not found."
        echo "  Install it with:  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        missing=1
    fi
    if ! command -v cc &>/dev/null && ! command -v gcc &>/dev/null; then
        local mgr; mgr="$(detect_pkg_manager)"
        echo "No C compiler/linker found (needed to link the binary)."
        case "$mgr" in
            apt)    echo "  Install with:  sudo apt-get install -y build-essential pkg-config" ;;
            dnf)        echo "  Install with:  sudo dnf install -y gcc pkgconf-pkg-config" ;;
            rpm-ostree) echo "  Install with:  sudo rpm-ostree install gcc pkgconf-pkg-config  (then reboot), or build inside a toolbox" ;;
            pacman) echo "  Install with:  sudo pacman -S --needed base-devel" ;;
            zypper) echo "  Install with:  sudo zypper install -y gcc pkg-config" ;;
            *)      echo "  Install a C toolchain for your distro." ;;
        esac
        missing=1
    fi
    if [ "$missing" -eq 1 ]; then
        echo; echo "Resolve the above, then re-run ./install.sh"; exit 1
    fi
}

refresh_caches() {
    command -v update-desktop-database &>/dev/null \
        && update-desktop-database "$APP_DIR" || true
    command -v gtk-update-icon-cache &>/dev/null \
        && gtk-update-icon-cache -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
}

# ─────────────────────────────────────────────────────────────────────────────
# Entry points
# ─────────────────────────────────────────────────────────────────────────────
case "${1:-}" in
    --uninstall)
        rm -f "$BIN" "$DESKTOP" "$ICON"
        refresh_caches
        echo "Uninstalled frms."
        exit 0
        ;;
    --check-deps)
        check_deps
        exit 0
        ;;
esac

SKIP_DEPS=0
[ "${1:-}" = "--skip-deps" ] && SKIP_DEPS=1

if [ "$SKIP_DEPS" -eq 0 ]; then
    check_deps
fi

check_build_deps

echo "── Building release binary ──────────────────────────────────────────"
cargo build --release --manifest-path "$SCRIPT_DIR/Cargo.toml"

echo "── Installing ───────────────────────────────────────────────────────"
install -Dm755 "$SCRIPT_DIR/target/release/frms" "$BIN"

# Render the icon PNG straight from the binary — same art as the window icon.
"$BIN" --export-icon "$ICON"

# Desktop entry. The filename (frms.desktop) and StartupWMClass must match
# the app's Wayland application_id ("frms", set in src/main.rs) so the shell
# can tie the running window to the pinned launcher.
install -d "$APP_DIR"
cat > "$DESKTOP" <<EOF
[Desktop Entry]
Type=Application
Name=frms
Comment=Cross-platform IDE
Exec=$BIN
Icon=frms
Terminal=false
Categories=Development;IDE;
StartupWMClass=frms
EOF

refresh_caches

echo
echo "Installed:"
echo "  $BIN"
echo "  $DESKTOP"
echo "  $ICON"
echo
echo "Open the app grid, search 'frms', launch it, then right-click"
echo "its dash icon and choose 'Pin to Dash'."
