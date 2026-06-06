# frms — Terminal IDE

A minimal, tmux-based terminal IDE using vim for editing and Claude Code for AI assistance.

## Layout

```
┌──────────┬──────────────────────────────┬──────────┐  ← 70% height
│  netrw   │           vim                │  shell   │
│  (20%)   │           (60%)              │  (20%)   │
├──────────┴──────────────────────────────┴──────────┤  ← 30% height
│         claude code          │      claude code    │
│            (50%)             │         (50%)       │
└──────────────────────────────┴─────────────────────┘
```

## Requirements

- `tmux`
- `vim`
- `claude` CLI — [Claude Code](https://claude.ai/code) (optional; falls back to a shell)

## Quick Start

```bash
git clone <repo-url> frms
cd frms
./setup.sh
```

`setup.sh` will:
1. Link `tmux.conf` to `~/.tmux.conf` (backs up any existing config).
2. Create a tmux session named `ide` with the layout above.
3. Attach to the session.

If the `ide` session already exists, it attaches directly without rebuilding the layout.

## Key Bindings

| Key | Action |
|-----|--------|
| `Ctrl-a` | Prefix (replaces default `Ctrl-b`) |
| `prefix r` | Reload tmux config |
| `prefix h/j/k/l` | Move between panes (vim-style) |
| `prefix H/J/K/L` | Resize pane |
| `prefix \|` | Split pane vertically |
| `prefix -` | Split pane horizontally |
| `prefix c` | New window in current path |

## Customization

- **Layout**: edit the pane-splitting section in `setup.sh`.
- **Key bindings / colors**: edit `tmux.conf`.
- **Startup commands**: change the `tmux send-keys` lines in `setup.sh` to launch different tools per pane.
