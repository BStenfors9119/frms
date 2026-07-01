#!/usr/bin/env bash
# package.sh — build a distributable frms package.
#
# The binary is self-contained except for glibc; runtime helper programs are
# declared as weak deps so the system installer pulls them in automatically:
#   .deb (apt)  → Recommends/Suggests   |   .rpm (dnf) → Recommends/Suggests
#
#   ./package.sh deb            build a .deb  (Debian / Ubuntu / Mint)
#   ./package.sh rpm            build an .rpm (Fedora / dnf)
#   ./package.sh all            build every format whose tooling is present
#                               (skips formats with missing tools, no error) —
#                               run it as-is in each toolbox to collect both
#   ./package.sh <fmt> --skip-build   reuse an existing target/release/frms
#
# Output lands in ./dist/.
#
# Notes:
#   • Format ≠ architecture. Both packages wrap the SAME x86_64 binary — no
#     cross-compilation. .deb won't install on Fedora and .rpm won't install
#     on Mint; that's a packaging-format difference, nothing more.
#   • glibc: build the .rpm on Fedora for Fedora (glibc matches). A .deb built
#     on a newer-glibc box (e.g. Fedora) may fail to START on an older Mint /
#     Ubuntu LTS — build the .deb on a matching-or-older box if so.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

FMT="${1:-}"
SKIP_BUILD=0
[ "${2:-}" = "--skip-build" ] && SKIP_BUILD=1

case "$FMT" in
    deb|rpm|all) ;;
    *) echo "usage: ./package.sh deb|rpm|all [--skip-build]"; exit 1 ;;
esac

VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
DIST="$SCRIPT_DIR/dist"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

# ── pre-flight: required packaging tools for the chosen format(s) ────────────
#
# `all` is a best-effort request: build every format whose tooling is present
# and skip (with a warning) the ones that aren't. This lets you run the SAME
# `./package.sh all` in each environment — the Debian toolbox emits the .deb,
# Fedora emits the .rpm — instead of needing both tools side-by-side. Asking
# for a single explicit format with its tool missing is still a hard error.
need_deb=0; need_rpm=0
[ "$FMT" = deb ] || [ "$FMT" = all ] && need_deb=1
[ "$FMT" = rpm ] || [ "$FMT" = all ] && need_rpm=1

if [ "$need_deb" = 1 ] && ! command -v dpkg-deb >/dev/null 2>&1; then
    if [ "$FMT" = all ]; then
        echo "⚠ dpkg-deb not found — skipping .deb."
        echo "  (to include it:  Fedora: sudo dnf install -y dpkg   Debian: sudo apt-get install -y dpkg-dev)"
        need_deb=0
    else
        echo "dpkg-deb not found — needed for .deb."
        echo "  Fedora:  sudo dnf install -y dpkg     Debian:  sudo apt-get install -y dpkg-dev"
        exit 1
    fi
fi
if [ "$need_rpm" = 1 ] && ! command -v rpmbuild >/dev/null 2>&1; then
    if [ "$FMT" = all ]; then
        echo "⚠ rpmbuild not found — skipping .rpm.  (to include it:  sudo dnf install -y rpm-build)"
        need_rpm=0
    else
        echo "rpmbuild not found — needed for .rpm.  Install:  sudo dnf install -y rpm-build"
        exit 1
    fi
fi

if [ "$need_deb" = 0 ] && [ "$need_rpm" = 0 ]; then
    echo "No packaging tools available — install dpkg (.deb) or rpm-build (.rpm) and retry."
    exit 1
fi

# ── build + stage the shared payload once ────────────────────────────────────
if [ "$SKIP_BUILD" -eq 0 ]; then
    echo "── Building release binary ──────────────────────────────────────────"
    cargo build --release
fi
BIN="$SCRIPT_DIR/target/release/frms"
[ -f "$BIN" ] || { echo "Missing $BIN — build first (drop --skip-build)."; exit 1; }

echo "── Staging package tree ─────────────────────────────────────────────"
install -Dm755 "$BIN" "$STAGE/usr/bin/frms"
STAGED_BIN="$STAGE/usr/bin/frms"

# The linker can bake the build machine's rust-toolchain lib dirs (and a temp
# `raw-dylibs` dir) into the binary's RUNPATH. Those paths don't exist on the
# target — harmless at load time, but Fedora's rpmbuild `check-rpaths` treats
# them as an invalid runpath and aborts the build. Strip it on the *staged*
# copy so the original target/release/frms is left untouched.
if command -v patchelf >/dev/null 2>&1; then
    patchelf --remove-rpath "$STAGED_BIN" 2>/dev/null || true
elif command -v chrpath >/dev/null 2>&1; then
    chrpath -d "$STAGED_BIN" 2>/dev/null || true
else
    echo "  ⚠ patchelf/chrpath not found — cannot strip the binary's RUNPATH."
    echo "    Install one to ship a clean package:  sudo dnf install -y patchelf"
    echo "    (the .rpm build will fall back to QA_RPATHS to avoid hard-failing.)"
fi

"$STAGED_BIN" --export-icon \
    "$STAGE/usr/share/icons/hicolor/256x256/apps/frms.png"
install -d "$STAGE/usr/share/applications"
cat > "$STAGE/usr/share/applications/frms.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=frms
Comment=Cross-platform IDE
Exec=/usr/bin/frms
Icon=frms
Terminal=false
Categories=Development;IDE;
StartupWMClass=frms
EOF

# Ship the Claude Code setup helper as `frms-setup-claude`. Build/agent panes
# need the npm-distributed `claude` CLI, which a distro package can't pull in;
# this one-shot command installs Node.js/npm + Claude Code and offers sign-in.
install -Dm755 "$SCRIPT_DIR/scripts/setup-claude.sh" "$STAGE/usr/bin/frms-setup-claude"

mkdir -p "$DIST"

# ─────────────────────────────────────────────────────────────────────────────
build_deb() {
    echo "── Building .deb ────────────────────────────────────────────────────"
    local debroot="$STAGE/.debroot"
    rm -rf "$debroot"
    mkdir -p "$debroot"
    cp -a "$STAGE/usr" "$debroot/"
    local size_kb; size_kb="$(du -ks "$debroot/usr" | cut -f1)"

    install -d "$debroot/DEBIAN"
    cat > "$debroot/DEBIAN/control" <<EOF
Package: frms
Version: $VERSION
Architecture: amd64
Maintainer: Bryan <bstenfors@sodapopsystems.com>
Installed-Size: $size_kb
Depends: libc6
Recommends: curl, xclip, chromium | chromium-browser, ffmpeg, openssh-client
Suggests: yt-dlp, sshpass
Section: devel
Priority: optional
Description: frms — cross-platform IDE
 A native Rust/Iced IDE with a file browser, editor, and embedded
 agent/terminal panes. Recommends bring in clipboard, networking, and
 in-app web-preview helpers.
EOF

    # NDA acceptance is handled in-app on first run (cross-format: rpm/deb/bare
    # binary), so no debconf install gate here.
    cat > "$debroot/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
command -v update-desktop-database >/dev/null 2>&1 && \
    update-desktop-database -q /usr/share/applications || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && \
    gtk-update-icon-cache -tq /usr/share/icons/hicolor || true
if ! command -v claude >/dev/null 2>&1; then
    echo "frms: Build/agent panes need the Claude Code CLI (not a distro package)."
    echo "      Run once to install it:  frms-setup-claude"
fi
EOF
    cp "$debroot/DEBIAN/postinst" "$debroot/DEBIAN/postrm"
    chmod 755 "$debroot/DEBIAN/postinst" "$debroot/DEBIAN/postrm"

    local out="$DIST/frms_${VERSION}_amd64.deb"
    dpkg-deb --build --root-owner-group "$debroot" "$out" >/dev/null
    echo "  → $out"

    local glibc_req
    glibc_req="$(objdump -T "$BIN" 2>/dev/null \
        | grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sort -V | tail -1 || true)"
    [ -n "$glibc_req" ] && echo "  (binary needs up to $glibc_req — recipient's glibc must be >= that)"
    echo "  install:  sudo apt install ./frms_${VERSION}_amd64.deb"
}

# ─────────────────────────────────────────────────────────────────────────────
build_rpm() {
    echo "── Building .rpm ────────────────────────────────────────────────────"
    local top="$STAGE/.rpmtop"
    rm -rf "$top"
    mkdir -p "$top"/{BUILD,RPMS,SPECS,SRPMS,BUILDROOT}
    local spec="$top/SPECS/frms.spec"
    cat > "$spec" <<'EOF'
Name:           frms
Version:        %{ver}
Release:        1%{?dist}
Summary:        frms — cross-platform IDE
License:        Proprietary
BuildArch:      x86_64
Requires:       glibc
Recommends:     curl
Recommends:     wl-clipboard
Recommends:     chromium
Recommends:     openssh-clients
Suggests:       ffmpeg
Suggests:       yt-dlp
Suggests:       sshpass

%description
A native Rust/Iced IDE with a file browser, editor, and embedded
agent/terminal panes. Weak dependencies bring in clipboard, networking,
and in-app web-preview helpers.

%install
cp -a %{stageroot}/usr %{buildroot}/

%files
/usr/bin/frms
/usr/bin/frms-setup-claude
/usr/share/applications/frms.desktop
/usr/share/icons/hicolor/256x256/apps/frms.png

%post
command -v update-desktop-database >/dev/null 2>&1 && \
    update-desktop-database -q /usr/share/applications || :
command -v gtk-update-icon-cache >/dev/null 2>&1 && \
    gtk-update-icon-cache -tq /usr/share/icons/hicolor || :
if ! command -v claude >/dev/null 2>&1; then
    echo "frms: Build/agent panes need the Claude Code CLI (not a distro package)."
    echo "      Run once to install it:  frms-setup-claude"
fi

%postun
command -v update-desktop-database >/dev/null 2>&1 && \
    update-desktop-database -q /usr/share/applications || :
command -v gtk-update-icon-cache >/dev/null 2>&1 && \
    gtk-update-icon-cache -tq /usr/share/icons/hicolor || :
EOF
    # QA_RPATHS bypasses check-rpaths' standard/invalid/empty RPATH errors. We
    # already strip the RUNPATH above when patchelf/chrpath is available; this
    # is the fallback so the build still succeeds when neither tool is present
    # (the leftover paths are inert on the target).
    QA_RPATHS="$(( 0x0001 | 0x0002 | 0x0010 ))" \
    rpmbuild -bb "$spec" \
        --define "_topdir $top" \
        --define "ver $VERSION" \
        --define "stageroot $STAGE" \
        >/dev/null
    local rpm_out; rpm_out="$(find "$top/RPMS" -name '*.rpm' | head -1)"
    cp "$rpm_out" "$DIST/"
    echo "  → $DIST/$(basename "$rpm_out")"
    echo "  install:  sudo dnf install ./$(basename "$rpm_out")"
    echo "  (ffmpeg is a Suggest — full ffmpeg lives in RPM Fusion, not base Fedora.)"
}

# ─────────────────────────────────────────────────────────────────────────────
[ "$need_deb" = 1 ] && build_deb
[ "$need_rpm" = 1 ] && build_rpm

echo
echo "Done. Packages in $DIST/"
