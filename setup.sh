#!/usr/bin/env bash
# setup.sh — bootstrap the frms terminal IDE

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SESSION="ide"

# ── package manager detection ─────────────────────────────────────────────────

detect_pkg_manager() {
    if command -v apt-get &>/dev/null; then echo "apt"
    elif command -v dnf &>/dev/null;     then echo "dnf"
    elif command -v pacman &>/dev/null;  then echo "pacman"
    elif command -v brew &>/dev/null;    then echo "brew"
    else echo ""
    fi
}

install_pkg() {
    local pkg="$1"
    local mgr
    mgr="$(detect_pkg_manager)"

    if [ -z "$mgr" ]; then
        echo "ERROR: No supported package manager found. Please install '$pkg' manually."
        exit 1
    fi

    read -r -p "  Install '$pkg' via $mgr? [y/N] " ans
    case "$ans" in
        [yY][eE][sS]|[yY])
            case "$mgr" in
                apt)    sudo apt-get install -y "$pkg" ;;
                dnf)    sudo dnf install -y "$pkg" ;;
                pacman) sudo pacman -S --noconfirm "$pkg" ;;
                brew)   brew install "$pkg" ;;
            esac
            ;;
        *)
            echo "Skipping '$pkg'. Cannot continue without it."
            exit 1
            ;;
    esac
}

# ── dependency check ──────────────────────────────────────────────────────────

check_dep() {
    local cmd="$1"
    local pkg="${2:-$1}"   # optional package name if different from command
    if ! command -v "$cmd" &>/dev/null; then
        echo "  '$cmd' is not installed."
        install_pkg "$pkg"
    fi
}

check_dep tmux
check_dep vim
check_dep fzf

# Optional: offer to install Claude Code CLI if missing
if ! command -v claude &>/dev/null; then
    echo "  'claude' (Claude Code CLI) is not installed."
    read -r -p "  Install Claude Code via npm? [y/N] " ans
    case "$ans" in
        [yY][eE][sS]|[yY])
            if command -v npm &>/dev/null; then
                npm install -g @anthropic-ai/claude-code
                CLAUDE_CMD="claude"
            else
                echo "  npm not found. Install Node.js first, then run: npm install -g @anthropic-ai/claude-code"
                CLAUDE_CMD="bash"
            fi
            ;;
        *)
            echo "  Skipping claude install. Bottom panes will open a shell instead."
            CLAUDE_CMD="bash"
            ;;
    esac
else
    CLAUDE_CMD="claude"
fi

# ── tmux config ───────────────────────────────────────────────────────────────

# Install tmux.conf as ~/.tmux.conf (back up existing if present)
if [ -f "$HOME/.tmux.conf" ] && [ ! -L "$HOME/.tmux.conf" ]; then
    echo "Backing up existing ~/.tmux.conf to ~/.tmux.conf.bak"
    cp "$HOME/.tmux.conf" "$HOME/.tmux.conf.bak"
fi
ln -sf "$SCRIPT_DIR/tmux.conf" "$HOME/.tmux.conf"
echo "Linked tmux.conf -> ~/.tmux.conf"

# ── launch IDE layout ─────────────────────────────────────────────────────────

if tmux has-session -t "$SESSION" 2>/dev/null; then
    echo "Session '$SESSION' already exists. Attaching..."
    tmux attach-session -t "$SESSION"
    exit 0
fi

# Use safe fallback dimensions (tput may return empty inside a toolbox container)
COLS=$(tput cols 2>/dev/null || true); COLS=${COLS:-220}
ROWS=$(tput lines 2>/dev/null || true); ROWS=${ROWS:-50}

# Create session — use pane IDs (%N) throughout to avoid base-index confusion
tmux new-session -d -s "$SESSION" -x "$COLS" -y "$ROWS"
tmux rename-window -t "$SESSION" "ide"

# Capture the initial pane ID
TOP=$(tmux display-message -t "$SESSION" -p '#{pane_id}')

# ── Row 1: top 70% — split into 2 vertical panes ─────────────────────────────
#
#   ┌──────────────────┬──────────────────────────────┐
#   │      left        │           middle             │
#   │      (40%)       │           (60%)              │
#   └──────────────────┴──────────────────────────────┘

# -P -F prints the new pane's ID immediately — no separate query needed
BOT_LEFT=$(tmux split-window -t "$TOP" -v -p 30 -P -F '#{pane_id}')
MID=$(tmux split-window      -t "$TOP" -h -p 60 -P -F '#{pane_id}')
LEFT=$TOP

# ── Row 2: bottom 30% — two equal panes for Claude Code ──────────────────────
#
#   ┌──────────────────────┬──────────────────────┐
#   │   claude (left 50%)  │  claude (right 50%)  │
#   └──────────────────────┴──────────────────────┘

BOT_RIGHT=$(tmux split-window -t "$BOT_LEFT" -h -p 50 -P -F '#{pane_id}')

# ── start tools in each pane ──────────────────────────────────────────────────

tmux send-keys -t "$LEFT"      "export FRMS_EDITOR_PANE=$MID && bash $SCRIPT_DIR/browser.sh" Enter
tmux send-keys -t "$MID"       "vim"          Enter   # middle: editor
tmux send-keys -t "$BOT_LEFT"  "$CLAUDE_CMD" Enter   # bottom-left: claude
tmux send-keys -t "$BOT_RIGHT" "$CLAUDE_CMD" Enter   # bottom-right: claude

# Bind prefix+b to jump to the browser pane from anywhere in the session
tmux bind-key -T prefix b select-pane -t "$LEFT"

# Focus the middle (main coding) pane
tmux select-pane -t "$MID"

# ── attach ────────────────────────────────────────────────────────────────────
tmux attach-session -t "$SESSION"
