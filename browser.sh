#!/usr/bin/env bash
# browser.sh — fzf-powered file browser for the frms IDE left pane
#
# Environment variables (set by setup.sh):
#   FRMS_EDITOR_PANE   — tmux pane ID of the middle editor pane
#   FRMS_PROJECTS_DIR  — root dir to search for projects (default: $HOME)
#
# Key bindings:
#   f / Ctrl+f  — fuzzy find file anywhere in the current tree
#   d           — fuzzy change directory within current tree
#   p           — switch to a different project (finds git roots)
#   r / Ctrl+r  — refresh listing
#   q / Ctrl+c  — quit

set -uo pipefail

EDITOR_PANE="${FRMS_EDITOR_PANE:-}"
PROJECTS_DIR="${FRMS_PROJECTS_DIR:-$HOME}"

# ── helpers ───────────────────────────────────────────────────────────────────

open_in_editor() {
    local file
    file="$(realpath "$1")"
    if [ -n "$EDITOR_PANE" ]; then
        # Send :e <file> to the vim session in the middle pane
        tmux send-keys -t "$EDITOR_PANE" Escape ":e $file" Enter
        tmux select-pane -t "$EDITOR_PANE"
        # Return focus to browser after a moment
        sleep 0.1
        tmux select-pane -t "${TMUX_PANE:-}"
    else
        vim "$file"
    fi
}

draw() {
    clear
    printf '\033[1;34m── %s\033[0m\n' "$(pwd)"
    echo
    ls -1 --color=always --group-directories-first 2>/dev/null || ls -1
    echo
    printf '\033[2m f/Ctrl+f: find file   d: change dir   p: project   r: refresh   q: quit\033[0m\n'
}

find_file() {
    local file
    file=$(find . -type f -not -path '*/.git/*' 2>/dev/null \
        | sed 's|^\./||' \
        | fzf --height=90% --border=rounded \
              --prompt="  file > " \
              --preview='cat -- {}' \
              --preview-window=right:55%:wrap \
              --bind='ctrl-f:reload(find . -type f -not -path "*/.git/*" | sed "s|^\./||")' \
        ) || return 0
    [ -n "$file" ] && open_in_editor "$file"
}

change_dir() {
    local dir
    dir=$(find . -type d -not -path '*/.git/*' 2>/dev/null \
        | sed 's|^\./||' \
        | fzf --height=90% --border=rounded \
              --prompt="  cd > " \
        ) || return 0
    [ -n "$dir" ] && cd "$dir"
}

change_project() {
    local dir
    dir=$(find "$PROJECTS_DIR" -maxdepth 4 -type d -name '.git' 2>/dev/null \
        | sed 's|/.git$||' \
        | fzf --height=90% --border=rounded \
              --prompt="  project > " \
              --preview='ls -1 --color=always -- {}' \
              --preview-window=right:40% \
        ) || return 0
    [ -n "$dir" ] && cd "$dir"
}

# ── main loop ─────────────────────────────────────────────────────────────────

draw
while true; do
    # Read a single keypress; -s = silent, -N1 = exactly 1 char
    IFS= read -r -s -N1 key || { draw; continue; }

    # Handle multi-byte escape sequences (arrow keys etc.) — discard them
    if [ "$key" = $'\x1b' ]; then
        read -r -s -N2 -t 0.05 _ 2>/dev/null || true
        draw
        continue
    fi

    case "$key" in
        f|$'\x06')   find_file;      draw ;;   # f or Ctrl+f
        d)           change_dir;     draw ;;
        p)           change_project; draw ;;
        r|$'\x12')   draw            ;;        # r or Ctrl+r
        q|$'\x03')   clear; exit 0   ;;        # q or Ctrl+c
    esac
done
