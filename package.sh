#!/usr/bin/env bash
# package.sh — build distributable frms packages (.rpm and Windows .exe zip).
#
# The .deb is intentionally NOT built here — it's owned by _package/_release.sh
# (its multi-glibc zigbuild + apt pipeline). This script covers the other two:
#
#   ./package.sh rpm            build an .rpm (Fedora / dnf)
#   ./package.sh exe            build a Windows .zip (frms.exe + installer),
#                               cross-compiled via cargo-xwin (MSVC target)
#   ./package.sh all            build every format whose tooling is present
#                               (skips a format with missing tools, no error)
#   ./package.sh <fmt> --bump=patch   bump the version (major | minor | patch)
#                                     before building, so the artifacts + GitHub
#                                     tag use the new number
#   ./package.sh <fmt> --skip-build   reuse an already-built binary
#   ./package.sh rpm --publish        also publish the built artifacts to a
#                                     GitHub Release (the Fedora .rpm + Windows
#                                     .zip download channel)
#
# Version source: the SAME place as _package/_release.sh — the `Version:` field
# of _package/frms/DEBIAN/control — so a --bump here stays in lock-step with the
# .deb pipeline. --bump edits that file only (no git commit/tag).
#
# Naming: every artifact is  frms_<version>_amd64.<ext>  — matching the .deb
# produced by _release.sh (frms_<version>_amd64.deb).
#
# --publish uploads this version's dist/ .rpm + .zip to a GitHub Release via the
# `gh` CLI (run `gh auth login` once). Tag defaults to v<version>; override with
# FRMS_RELEASE_TAG. Re-running clobbers same-named assets.
#
# Output lands in ./dist/.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

FMT="${1:-}"
shift || true
SKIP_BUILD=0
PUBLISH=0
BUMP=""
for arg in "$@"; do
    case "$arg" in
        --skip-build) SKIP_BUILD=1 ;;
        --publish)    PUBLISH=1 ;;
        --bump=*)     BUMP="${arg#--bump=}" ;;
        *) echo "unknown option: $arg"; exit 1 ;;
    esac
done

case "$FMT" in
    rpm|exe|all) ;;
    *) echo "usage: ./package.sh rpm|exe|all [--bump=major|minor|patch] [--skip-build] [--publish]"; exit 1 ;;
esac

# The version lives in the same control file _release.sh reads/bumps, keeping the
# .rpm/.exe in lock-step with the .deb.
CONTROL="$SCRIPT_DIR/_package/frms/DEBIAN/control"
[ -f "$CONTROL" ] || { echo "version source not found: $CONTROL (same file _release.sh uses)."; exit 1; }

# Optionally bump the semver in the control file *before* reading it, so the
# build, artifact names, and GitHub tag all use the new version. Mirrors
# _package/_release.sh's bump (file edit only — no git commit/tag here).
if [ -n "$BUMP" ]; then
    cur="$(grep -m1 '^Version:' "$CONTROL" | awk '{print $2}')"
    echo "$cur" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$' \
        || { echo "current version '$cur' in $CONTROL is not semver (major.minor.patch)."; exit 1; }
    IFS=. read -r MA MI PA <<< "$cur"
    case "$BUMP" in
        major) MA=$((MA + 1)); MI=0; PA=0 ;;
        minor) MI=$((MI + 1)); PA=0 ;;
        patch) PA=$((PA + 1)) ;;
        *) echo "--bump must be major, minor, or patch (got '$BUMP')."; exit 1 ;;
    esac
    NEW="$MA.$MI.$PA"
    sed -i "s/^Version: .*/Version: $NEW/" "$CONTROL"
    echo "Bumped version: $cur → $NEW  ($CONTROL; commit + tag when ready)"
fi

VERSION="$(grep -m1 '^Version:' "$CONTROL" | awk '{print $2}')"
[ -n "$VERSION" ] || { echo "could not read Version: from $CONTROL."; exit 1; }
DIST="$SCRIPT_DIR/dist"
WIN_TARGET="x86_64-pc-windows-msvc"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

# ── pre-flight: required packaging tools for the chosen format(s) ────────────
#
# `all` is a best-effort request: build every format whose tooling is present
# and skip (with a warning) the ones that aren't. A single explicit format with
# its tool missing is still a hard error.
need_rpm=0; need_exe=0
[ "$FMT" = rpm ] || [ "$FMT" = all ] && need_rpm=1
[ "$FMT" = exe ] || [ "$FMT" = all ] && need_exe=1

if [ "$need_rpm" = 1 ] && ! command -v rpmbuild >/dev/null 2>&1; then
    if [ "$FMT" = all ]; then
        echo "⚠ rpmbuild not found — skipping .rpm.  (to include it:  sudo dnf install -y rpm-build)"
        need_rpm=0
    else
        echo "rpmbuild not found — needed for .rpm.  Install:  sudo dnf install -y rpm-build"
        exit 1
    fi
fi

# The Windows .exe needs cargo-xwin, the zip tool, and the MSVC Rust target.
if [ "$need_exe" = 1 ]; then
    exe_missing=""
    command -v cargo-xwin >/dev/null 2>&1 || exe_missing="cargo-xwin"
    command -v zip        >/dev/null 2>&1 || exe_missing="${exe_missing:+$exe_missing, }zip"
    rustup target list --installed 2>/dev/null | grep -qx "$WIN_TARGET" \
        || exe_missing="${exe_missing:+$exe_missing, }rust target $WIN_TARGET"
    if [ -n "$exe_missing" ]; then
        hint="cargo install cargo-xwin ; rustup target add $WIN_TARGET ; sudo dnf install -y zip"
        if [ "$FMT" = all ]; then
            echo "⚠ missing for .exe: $exe_missing — skipping .exe.  (to include it:  $hint)"
            need_exe=0
        else
            echo "missing for .exe: $exe_missing"
            echo "  install:  $hint"
            exit 1
        fi
    fi
fi

if [ "$need_rpm" = 0 ] && [ "$need_exe" = 0 ]; then
    echo "No packaging tools available — install rpm-build (.rpm) or cargo-xwin (.exe)."
    exit 1
fi

# ── build + stage the Linux payload (rpm only; exe builds separately) ────────
if [ "$need_rpm" = 1 ]; then
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
fi  # ── end rpm staging ──

mkdir -p "$DIST"

# ─────────────────────────────────────────────────────────────────────────────
# Move a freshly built artifact into dist/latest/, first archiving any prior
# artifact of the SAME type (extension) — from latest/ and from the old flat
# dist/ root — into dist/archived/. So dist/latest/ always holds the newest
# .rpm and .zip, and dist/archived/ keeps the older ones. Prints the final path.
# `src` must live OUTSIDE dist/ (we stage in $STAGE) so the sweep can't catch it.
place_artifact() {
    local src="$1"
    local ext="${src##*.}"
    local latest="$DIST/latest" archived="$DIST/archived"
    mkdir -p "$latest" "$archived"
    local f
    for f in "$latest"/*."$ext" "$DIST"/*."$ext"; do
        [ -e "$f" ] || continue
        mv -f "$f" "$archived/"
    done
    mv -f "$src" "$latest/"
    printf '%s\n' "$latest/$(basename "$src")"
}

# ─────────────────────────────────────────────────────────────────────────────
build_rpm() {
    echo "── Building .rpm ────────────────────────────────────────────────────"
    local top="$STAGE/.rpmtop"
    rm -rf "$top"
    mkdir -p "$top"/{BUILD,RPMS,SPECS,SRPMS,BUILDROOT}
    local spec="$top/SPECS/frms.spec"
    cat > "$spec" <<'EOF'
# Prebuilt binary — no source, so no -debuginfo subpackage to extract.
%global debug_package %{nil}
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
    # A minimal changelog silences rpmbuild's "source_date_epoch is set but
    # %changelog has no entries" warning, and is good spec hygiene.
    local today; today="$(date +'%a %b %d %Y')"
    cat >> "$spec" <<EOF

%changelog
* $today Bryan <bstenfors@sodapopsystems.com> - $VERSION-1
- Automated package build.
EOF

    # Route rpmbuild's (verbose, stderr) output to a log and surface it only on
    # failure, so a successful build stays quiet. QA_RPATHS bypasses check-rpaths'
    # standard/invalid/empty RPATH errors — we already strip the RUNPATH above
    # when patchelf/chrpath is present; this is the fallback when neither is (the
    # leftover paths are inert on the target).
    local log="$top/rpmbuild.log"
    if ! QA_RPATHS="$(( 0x0001 | 0x0002 | 0x0010 ))" \
        rpmbuild -bb "$spec" \
            --define "_topdir $top" \
            --define "ver $VERSION" \
            --define "stageroot $STAGE" \
            >"$log" 2>&1; then
        echo "  ✗ rpmbuild failed — last lines:"
        tail -25 "$log" | sed 's/^/    /'
        return 1
    fi
    local rpm_out; rpm_out="$(find "$top/RPMS" -name '*.rpm' | head -1)"
    # Rename to the shared convention: frms_<version>_amd64.rpm (the internal
    # rpm metadata still carries the correct x86_64 arch; the filename is
    # cosmetic and dnf installs by metadata, not by name). Stage in $STAGE, then
    # place into dist/latest/ (archiving any prior .rpm).
    cp "$rpm_out" "$STAGE/frms_${VERSION}_amd64.rpm"
    local out; out="$(place_artifact "$STAGE/frms_${VERSION}_amd64.rpm")"
    echo "  → $out"
    echo "  install:  sudo dnf install ./$(basename "$out")"
    echo "  (ffmpeg is a Suggest — full ffmpeg lives in RPM Fusion, not base Fedora.)"
}

# ─────────────────────────────────────────────────────────────────────────────
build_exe() {
    echo "── Building Windows .exe (zip) ──────────────────────────────────────"
    # Statically link the MSVC C runtime (+crt-static) so frms.exe runs on a
    # fresh Windows box with no VC++ redist. The icon is embedded by build.rs
    # when a resource compiler (llvm-rc) is present; otherwise the app falls back
    # to its runtime window icon (non-fatal).
    if [ "$SKIP_BUILD" -eq 0 ]; then
        RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C target-feature=+crt-static" \
            cargo xwin build --release --target "$WIN_TARGET"
    fi
    local exe="$SCRIPT_DIR/target/$WIN_TARGET/release/frms.exe"
    [ -f "$exe" ] || { echo "  ✗ frms.exe not found — build first (drop --skip-build)."; return 1; }

    # Stage the payload: the self-contained exe, docs, and the PS installer.
    local win_stage; win_stage="$(mktemp -d)"
    cp "$exe" "$win_stage/"
    [ -f "$SCRIPT_DIR/README.md" ] && cp "$SCRIPT_DIR/README.md" "$win_stage/"
    if [ -f "$SCRIPT_DIR/_scripts/install-frms.ps1" ]; then
        mkdir -p "$win_stage/_scripts"
        cp "$SCRIPT_DIR/_scripts/install-frms.ps1" "$win_stage/_scripts/"
    fi

    # Zip into $STAGE, then place into dist/latest/ (archiving any prior .zip).
    local built="$STAGE/frms_${VERSION}_amd64.zip"
    rm -f "$built"
    ( cd "$win_stage" && zip -rq "$built" . )
    rm -rf "$win_stage"
    local out; out="$(place_artifact "$built")"
    echo "  → $out"
    echo "  install on Windows:  powershell -ExecutionPolicy Bypass -File .\\_scripts\\install-frms.ps1"
}

# ─────────────────────────────────────────────────────────────────────────────
# Publish this version's dist/ artifacts (.rpm — the Fedora channel — and the
# Windows .zip) to a GitHub Release via the gh CLI.
publish_github() {
    command -v gh >/dev/null 2>&1 || {
        echo "  ✗ gh not found (needed to publish).  sudo dnf install -y gh ; gh auth login"; return 1; }
    gh auth status >/dev/null 2>&1 || { echo "  ✗ gh not authenticated — run:  gh auth login"; return 1; }

    local tag="${FRMS_RELEASE_TAG:-v$VERSION}"
    local assets=() f
    for f in "$DIST/latest"/frms_"${VERSION}"_amd64.rpm "$DIST/latest"/frms_"${VERSION}"_amd64.zip; do
        if [ -f "$f" ]; then assets+=("$f"); fi
    done
    if [ "${#assets[@]}" -eq 0 ]; then
        echo "⚠ --publish: no frms $VERSION .rpm/.zip in dist/latest/ to upload (build them first)."
        return 0
    fi

    echo "── Publishing $tag to GitHub with ${#assets[@]} asset(s) ────────────────"
    printf '     • %s\n' "${assets[@]##*/}"
    if gh release view "$tag" >/dev/null 2>&1; then
        gh release upload "$tag" "${assets[@]}" --clobber
    else
        gh release create "$tag" "${assets[@]}" \
            --title "frms $VERSION" \
            --notes "frms $VERSION (amd64 / x86_64).

* Fedora: install the .rpm with \`sudo dnf install ./<file>.rpm\`.
* Windows: unzip and run \`_scripts\\install-frms.ps1\`.
* Debian/Ubuntu/Mint: add the SodaPop apt repo, or grab the .deb (see README)."
    fi
    echo "  → $(gh release view "$tag" --json url -q .url 2>/dev/null || echo "$tag")"
}

# ─────────────────────────────────────────────────────────────────────────────
[ "$need_rpm" = 1 ] && build_rpm
[ "$need_exe" = 1 ] && build_exe

[ "$PUBLISH" = 1 ] && publish_github

echo
echo "Done. Packages in $DIST/"
