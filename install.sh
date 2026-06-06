#!/usr/bin/env bash
# install.sh — install frms as a desktop application (user-local).
#
# Fedora Silverblue friendly: everything lands under ~/.local, so no
# rpm-ostree layering and no reboot. After install, frms appears in the
# GNOME app grid and can be pinned to the dash/taskbar.
#
#   ./install.sh              build (release) and install
#   ./install.sh --uninstall  remove the installed files

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

BIN_DIR="$HOME/.local/bin"
APP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/256x256/apps"

BIN="$BIN_DIR/frms"
DESKTOP="$APP_DIR/frms.desktop"
ICON="$ICON_DIR/frms.png"

refresh_caches() {
    command -v update-desktop-database &>/dev/null \
        && update-desktop-database "$APP_DIR" || true
    command -v gtk-update-icon-cache &>/dev/null \
        && gtk-update-icon-cache -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$BIN" "$DESKTOP" "$ICON"
    refresh_caches
    echo "Uninstalled frms."
    exit 0
fi

echo "── Building release binary ──────────────────────────────────────────"
cargo build --release --manifest-path "$SCRIPT_DIR/Cargo.toml"

echo "── Installing ───────────────────────────────────────────────────────"
install -Dm755 "$SCRIPT_DIR/target/release/frms" "$BIN"

# Render the icon PNG straight from the binary — same art as the window icon.
"$BIN" --export-icon "$ICON"

# Desktop entry. The filename (frms.desktop) and StartupWMClass must match
# the app's Wayland application_id ("frms", set in src/main.rs) so GNOME can
# tie the running window to the pinned launcher.
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
echo "Open the GNOME app grid, search 'frms', launch it, then right-click"
echo "its dash icon and choose 'Pin to Dash'."
