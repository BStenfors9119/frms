#!/usr/bin/env bash
# setup-claude.sh — install the Claude Code CLI that frms' Build/agent panes use.
#
# Claude Code ships via npm, not as a distro package, so an .rpm/.deb install of
# frms can't pull it in automatically. This one-shot helper does the whole setup:
#   1. installs Node.js + npm (via the system package manager) if missing,
#   2. installs Claude Code globally (npm install -g @anthropic-ai/claude-code),
#   3. offers the one-time interactive sign-in.
#
# It ships in the package as `frms-setup-claude` and is also invoked by
# install.sh, so this file is the single source of truth for the flow.
#
# Re-running it is safe: present components are detected and skipped.

set -euo pipefail

CLAUDE_PKG="@anthropic-ai/claude-code"

detect_pkg_manager() {
    if   command -v apt-get    &>/dev/null; then echo apt
    elif command -v dnf        &>/dev/null; then echo dnf
    elif command -v rpm-ostree &>/dev/null; then echo rpm-ostree
    elif command -v pacman     &>/dev/null; then echo pacman
    elif command -v zypper     &>/dev/null; then echo zypper
    elif command -v brew       &>/dev/null; then echo brew
    else echo ""; fi
}

# Distro package(s) providing node + npm for the given manager.
node_pkgs() {
    case "$1" in
        apt|dnf|rpm-ostree|zypper|pacman) echo "nodejs npm" ;;
        brew)                             echo "node" ;;
        *)                                echo "" ;;
    esac
}

install_pkgs() {
    local mgr="$1"; shift
    case "$mgr" in
        apt)        sudo apt-get update -qq && sudo apt-get install -y "$@" ;;
        dnf)        sudo dnf install -y "$@" ;;
        rpm-ostree) sudo rpm-ostree install --idempotent --allow-inactive "$@" ;;
        pacman)     sudo pacman -S --needed --noconfirm "$@" ;;
        zypper)     sudo zypper --non-interactive install "$@" ;;
        brew)       brew install "$@" ;;
    esac
}

echo "── Claude Code setup ────────────────────────────────────────────────"

if command -v claude &>/dev/null; then
    echo "Claude Code is already installed ($(command -v claude)). ✓"
else
    mgr="$(detect_pkg_manager)"

    # Ensure Node.js / npm first — Claude Code is an npm global.
    if ! command -v npm &>/dev/null; then
        pkgs="$(node_pkgs "$mgr")"
        if [ -z "$mgr" ] || [ -z "$pkgs" ]; then
            echo "Node.js/npm not found and no known package manager to install them."
            echo "Install Node 18+ manually, then:  sudo npm install -g $CLAUDE_PKG"
            exit 1
        fi
        echo "Node.js/npm not found — installing them first ($pkgs)…"
        install_pkgs "$mgr" $pkgs
        if [ "$mgr" = rpm-ostree ] && ! command -v npm &>/dev/null; then
            echo
            echo "⚠ rpm-ostree layered Node.js but it won't be active until you reboot."
            echo "  Reboot, then re-run:  frms-setup-claude"
            exit 1
        fi
    fi

    echo "Installing Claude Code:  npm install -g $CLAUDE_PKG"
    # Default npm global prefix (/usr or /usr/local) needs root; Homebrew's is
    # user-writable, so skip sudo there.
    if [ "$mgr" = brew ]; then
        npm install -g "$CLAUDE_PKG"
    else
        sudo npm install -g "$CLAUDE_PKG"
    fi

    if command -v claude &>/dev/null; then
        echo "Claude Code installed ($(command -v claude)). ✓"
    else
        echo "⚠ 'claude' still isn't on PATH — open a new shell, then run: claude"
        exit 1
    fi
fi

# One-time sign-in (Pro/Max account or API credits). Interactive, so offer it
# rather than assuming. Skip the prompt when not on a terminal (e.g. piped).
echo
if [ -t 0 ]; then
    read -r -p "Sign in to Claude now (opens the login flow)? [Y/n] " ans
    case "${ans:-Y}" in
        [nN]*) echo "Skipped — run 'claude' once to sign in before using Build panes." ;;
        *)     claude || echo "Sign-in didn't complete — run 'claude' again when ready." ;;
    esac
else
    echo "Run 'claude' once to sign in before using Build panes."
fi

echo
echo "Done. Build/agent panes in frms will now launch Claude Code."
echo "(Research/Chat panes use an Anthropic API key entered in frms' Profile tab.)"
