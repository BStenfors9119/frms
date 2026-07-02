//! Per-session shell terminals exposed through the Terminals plugin tab.
//!
//! Mirrors the shape of `NotesState`: a list with create/select/delete plus
//! an editable name for the current selection. PTYs themselves die with the
//! process, but terminals flagged `pinned` have their name persisted so a
//! fresh shell with the same name is respawned on next launch (rooted in
//! the session's working directory).

use std::path::Path;

use crate::terminal::{TerminalId, TerminalPane};

pub struct PluginTerminal {
    pub id:     TerminalId,
    pub name:   String,
    pub pane:   TerminalPane,
    pub pinned: bool,
}

pub struct TerminalsState {
    pub terminals:  Vec<PluginTerminal>,
    pub selected:   Option<TerminalId>,
    pub name_input: String,
    /// True when keyboard input should be routed to the currently-selected
    /// plugin terminal instead of the session's active Claude terminal.
    pub focused:    bool,
    /// True while the Name field holds keyboard focus (just after creating a
    /// terminal or while the user is editing its name). When set, Tab/Enter
    /// moves focus out of the Name field and into the terminal rather than
    /// sending a literal tab byte to the shell.
    pub naming:     bool,
}

impl TerminalsState {
    pub fn new() -> Self {
        Self {
            terminals:  Vec::new(),
            selected:   None,
            name_input: String::new(),
            focused:    false,
            naming:     false,
        }
    }

    /// Spawn a new shell-backed terminal under `id` rooted at `working_dir`.
    pub fn create(&mut self, id: TerminalId, working_dir: &Path) {
        let name = format!("Terminal {}", self.terminals.len() + 1);
        self.create_with_name(id, working_dir, name, false);
    }

    /// Spawn a terminal with a caller-supplied name and pin flag — used by
    /// the persistence restore path to bring back pinned terminals with the
    /// names the user gave them.
    pub fn create_with_name(
        &mut self,
        id:          TerminalId,
        working_dir: &Path,
        name:        String,
        pinned:      bool,
    ) {
        let (shell, args) = crate::port::shell::login_shell();
        let pane = match TerminalPane::spawn_args(id, &shell, &args, &[], Some(working_dir)) {
            Ok(p)  => p,
            Err(e) => { eprintln!("[terminals] spawn {shell} failed: {e}"); return; }
        };
        self.name_input = name.clone();
        self.terminals.push(PluginTerminal { id, name, pane, pinned });
        self.selected = Some(id);
        // A freshly-created terminal starts in naming mode: keyboard focus
        // sits in the Name field so the user can title it, then Tab/Enter to
        // drop into the shell.
        self.focused  = false;
        self.naming   = true;
    }

    /// Flip the pin state of a terminal. Pin doesn't lock — the user can
    /// still delete pinned terminals; pin only controls whether the name
    /// is restored on next launch.
    pub fn toggle_pin(&mut self, id: TerminalId) {
        if let Some(t) = self.terminals.iter_mut().find(|t| t.id == id) {
            t.pinned = !t.pinned;
        }
    }

    pub fn select(&mut self, id: TerminalId) {
        if self.selected == Some(id) { return; }
        if let Some(t) = self.terminals.iter().find(|t| t.id == id) {
            self.selected   = Some(id);
            self.name_input = t.name.clone();
            self.focused    = true;
            self.naming     = false;
        }
    }

    /// Move keyboard focus out of the Name field and into the selected
    /// terminal. Called when the user Tabs/Enters out of naming, or clicks
    /// the terminal canvas.
    pub fn focus_terminal(&mut self) {
        if self.selected.is_some() {
            self.naming  = false;
            self.focused = true;
        }
    }

    pub fn delete(&mut self, id: TerminalId) {
        let Some(pos) = self.terminals.iter().position(|t| t.id == id) else { return; };
        self.terminals.remove(pos);
        if self.selected == Some(id) {
            let fallback = pos.saturating_sub(1);
            if let Some(t) = self.terminals.get(fallback) {
                self.selected   = Some(t.id);
                self.name_input = t.name.clone();
            } else {
                self.selected   = None;
                self.name_input = String::new();
                self.focused    = false;
                self.naming     = false;
            }
        }
    }

    pub fn edit_name(&mut self, s: String) {
        self.name_input = s;
        // Typing in the Name field means the field has keyboard focus; Tab/Enter
        // should now jump into the terminal rather than emit a tab byte.
        self.naming = true;
        if let Some(id) = self.selected {
            if let Some(t) = self.terminals.iter_mut().find(|t| t.id == id) {
                t.name = self.name_input.clone();
            }
        }
    }

    pub fn terminal(&self, id: TerminalId) -> Option<&PluginTerminal> {
        self.terminals.iter().find(|t| t.id == id)
    }

    pub fn terminal_mut(&mut self, id: TerminalId) -> Option<&mut PluginTerminal> {
        self.terminals.iter_mut().find(|t| t.id == id)
    }

    pub fn has_terminal(&self, id: TerminalId) -> bool {
        self.terminals.iter().any(|t| t.id == id)
    }
}
