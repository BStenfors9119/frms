# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project
A cross-platform IDE built with Rust and Iced. No tmux dependency — all UI is native Iced widgets. The file browser and editor are first-class Iced components; embedded terminal panes (for Claude sessions) are placeholders pending a terminal widget.

## Tech Stack
- Rust + Iced 0.13 — entire UI, no tmux

## Commands
- `cargo build` — compile
- `cargo run` — run the IDE
- `cargo build --release` — optimised binary (`target/release/frms`)

## Architecture
```
src/
├── main.rs                  # Entry point — window size, launches Frms
├── app.rs                   # Frms state + Message + Application impl
│                            #   owns two TerminalPane instances (TERMINAL_LEFT/RIGHT)
│                            #   subscription() batches two PTY subscriptions + keyboard
├── layout.rs                # LayoutKind, Layout (owns pane_grid::State + configs)
├── pane.rs                  # PaneKind enum (FileBrowser | Editor | Claude)
├── file_browser.rs          # FileBrowserState — navigate(), entries
├── editor.rs                # EditorState — text_editor::Content, open()
├── terminal/
│   ├── mod.rs               # TerminalPane (PTY spawn, process(), write_input())
│   │                        #   pty_subscription() — reads PTY → TerminalData msgs
│   └── grid.rs              # Grid (80×24 cells) + VT100 Perform impl (vte crate)
└── ui/
    ├── header.rs            # Project name bar
    ├── tab_bar.rs           # Default / Focused tab switcher
    ├── file_browser.rs      # Scrollable dir listing; BrowseDir / OpenFile
    ├── editor.rs            # Monospace text_editor widget
    └── terminal.rs          # Canvas widget — renders Grid, handles mouse focus
```

### Terminal data flow
```
PTY master (portable-pty)
  └─ spawn_blocking read loop  ← pty_subscription()
       └─ Message::TerminalData(id, bytes)
            └─ app::update() → TerminalPane::process()
                 └─ vte::Parser::advance() → Grid (cells updated)
                      └─ view() → ui::terminal canvas re-renders
```

Keyboard input goes: `iced::Event::Keyboard` → `Message::KeyEvent` →
`keyboard_event_to_bytes()` → `TerminalPane::write_input()` → PTY writer.
Clicking a terminal canvas emits `Message::TerminalFocused(id)` to route
keyboard input to the right pane.

## Usage
1. `cargo run` or install the release binary

## IDE Session Layouts
- The default session layout will consist of:
  - 2 rows
  - first adjustable row will start at 60% height and will contain 2 panes (left [40% width], middle [60% width])
    - the left pane is the file browser, the middle pane is the main coding area.
  - second adjustable row will start at 40% height and will contain 2 panes (left [50% width], right [50% width])
    - Each pane will be an instance of claude.ai/code, allowing for multiple coding sessions or tools to be open simultaneously.
  - There will be a header at the top of the IDE that will display the project name and provide access to settings and other features.
  - Top left pane should be a file browser, allowing users to navigate their project files and open them in the main coding area.
  - The main coding area will support syntax highlighting, code completion, and other features to enhance the coding experience.
  - The bottom panes will be used for claude sessions
- A second layout option will be:
  - 2 rows
  - first adjustable row will start at 70% height and will contain 1 pane (100% width)
    - this pane will be the main coding area.
  - second adjustable row will start at 30% height and will contain 2 panes (left [50% width], right [50% width])
    - Each pane will be an instance of claude.ai/code, allowing for multiple coding sessions or tools to be open simultaneously.
  - The header will remain the same as in the default layout, providing access to project information and settings.
- Each session layout will be a tab in a tab control under the header