//! Per-session shell terminals exposed through the Terminals plugin tab.
//!
//! Mirrors the shape of `NotesState`: a list with create/select/delete plus
//! an editable name for the current selection. PTYs themselves die with the
//! process, but terminals flagged `pinned` have their name, last working
//! directory *and* the toolbox they were in persisted, so a fresh shell with
//! the same name is respawned on next launch rooted where the user left it
//! (falling back to the session's working directory if that path is gone or
//! the OS couldn't read it) and dropped back into the same container.

use std::path::Path;

use crate::terminal::{Container, TerminalId, TerminalPane};

/// What to type into a freshly-spawned shell before handing it to the user:
/// re-enter the container it was left in, then apply the user's `ls` flags.
///
/// These are written into the PTY as ordinary input rather than baked into the
/// spawn command, for two reasons. `toolbox enter` replaces the shell, so an
/// alias set beforehand wouldn't survive into the container — the order here
/// matters, and it's why the alias line comes second. And a login shell reads
/// its own rc files, so anything we set via the spawn command would be
/// overwritten by them anyway.
#[derive(Debug, Default, Clone)]
pub struct ShellInit {
    /// Container to re-enter, from the last time this terminal was saved.
    pub container: Option<Container>,
    /// `ls` short options to apply, already normalised by [`crate::prefs`] to
    /// the offered set (e.g. `"lha"`). Empty leaves `ls` alone.
    pub ls_flags: String,
}

impl ShellInit {
    /// The lines to type into the new shell, in order. Empty when there's
    /// nothing to set up, in which case nothing is written at all.
    fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(cmd) = self.container.as_ref().and_then(enter_command) {
            lines.push(cmd);
        }
        if let Some(alias) = self.ls_alias() {
            lines.push(alias);
        }
        lines
    }

    /// The `alias ls=…` line, or `None` when no flags are set.
    ///
    /// Both Fedora and Debian ship `alias ls='ls --color=auto'` for interactive
    /// shells, so defining `alias ls='ls -lha'` outright would silently kill
    /// coloured listings. Under bash we extend whatever alias is already in
    /// place instead (`ls --color=auto -lha`); `BASH_ALIASES` is bash-only, so
    /// any other shell falls back to the plain form and loses the merge.
    ///
    /// Every line is prefixed with a space so shells configured with
    /// `HISTCONTROL=ignorespace` keep it out of the user's history.
    fn ls_alias(&self) -> Option<String> {
        if self.ls_flags.is_empty() {
            return None;
        }
        let flags = &self.ls_flags;
        Some(if shell_is_bash() {
            format!(r#" alias ls="${{BASH_ALIASES[ls]:-ls}} -{flags}""#)
        } else {
            format!(" alias ls='ls -{flags}'")
        })
    }
}

/// Whether the user's login shell is bash — it decides which `ls` alias form
/// [`ShellInit::ls_alias`] can use. Containers are judged by the same shell:
/// `toolbox enter` always runs bash, and `distrobox enter` runs `$SHELL`.
fn shell_is_bash() -> bool {
    let (shell, _) = crate::port::shell::login_shell();
    shell.rsplit(['/', '\\']).next().unwrap_or(&shell).starts_with("bash")
}

/// The command that re-enters `container` interactively, or `None` when we
/// don't know how to (an unrecognised runtime, or a name that isn't a plain
/// container name — the name is interpolated into a shell line, and podman
/// only ever issues names from this character set).
fn enter_command(container: &Container) -> Option<String> {
    let tool = match container.runtime.as_str() {
        "toolbox"   => "toolbox",
        "distrobox" => "distrobox",
        _           => return None,
    };
    let name_ok = !container.name.is_empty()
        && container.name.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    name_ok.then(|| format!("{tool} enter {}", container.name))
}

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
    pub fn create(&mut self, id: TerminalId, working_dir: &Path, init: &ShellInit) {
        let name = format!("Terminal {}", self.terminals.len() + 1);
        self.create_with_name(id, working_dir, name, false, init);
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
        init:        &ShellInit,
    ) {
        let (shell, args) = crate::port::shell::login_shell();
        let pane = match TerminalPane::spawn_args(id, &shell, &args, &[], Some(working_dir)) {
            Ok(p)  => p,
            Err(e) => { eprintln!("[terminals] spawn {shell} failed: {e}"); return; }
        };
        // Type the setup lines straight in. It's safe to send them all at once
        // even though the first may replace the shell: a shell reads its input
        // a line at a time, so whatever it doesn't consume stays in the tty
        // buffer for whatever it exec'd — which is exactly how the `ls` alias
        // lands *inside* the toolbox rather than outside it.
        let lines = init.lines();
        if !lines.is_empty() {
            pane.write_input(format!("{}\n", lines.join("\n")).as_bytes());
        }
        self.name_input = name.clone();
        self.terminals.push(PluginTerminal { id, name, pane, pinned });
        self.selected = Some(id);
        // A freshly-created terminal starts in naming mode: keyboard focus
        // sits in the Name field so the user can title it, then Tab/Enter to
        // drop into the shell.
        self.focused  = false;
        self.naming   = true;
    }

    /// Spawn a terminal running an arbitrary `(program, args)` under `id`,
    /// rooted at `working_dir`, with a caller-supplied name. Used by the
    /// Profile tab's "Install" button to run a tool's install command in a
    /// visible shell. Never pinned — the install run is one-off, not restored.
    pub fn create_command(
        &mut self,
        id:          TerminalId,
        working_dir: &Path,
        name:        String,
        program:     &str,
        args:        &[String],
    ) {
        let pane = match TerminalPane::spawn_args(id, program, args, &[], Some(working_dir)) {
            Ok(p)  => p,
            Err(e) => { eprintln!("[terminals] spawn {program} failed: {e}"); return; }
        };
        self.name_input = name.clone();
        self.terminals.push(PluginTerminal { id, name, pane, pinned: false });
        self.selected = Some(id);
        // Drop straight into the running install — no naming step.
        self.focused = true;
        self.naming  = false;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn container(name: &str, runtime: &str) -> Container {
        Container { name: name.to_string(), runtime: runtime.to_string() }
    }

    #[test]
    fn the_toolbox_is_entered_before_the_alias_is_set() {
        let init = ShellInit {
            container: Some(container("bottle", "toolbox")),
            ls_flags:  "lha".to_string(),
        };
        let lines = init.lines();
        // Order is the whole point: the alias has to be defined by the shell
        // running *inside* the container, so it must come second.
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "toolbox enter bottle");
        assert!(lines[1].starts_with(" alias ls="), "got {:?}", lines[1]);
        assert!(lines[1].contains("-lha"));
    }

    #[test]
    fn the_bash_alias_extends_the_one_already_in_place() {
        // Distros alias `ls` to `ls --color=auto`; replacing that outright
        // would lose colour, so under bash the flags are appended to it.
        let init = ShellInit { container: None, ls_flags: "lha".to_string() };
        let line = init.ls_alias().unwrap();
        if shell_is_bash() {
            assert_eq!(line, r#" alias ls="${BASH_ALIASES[ls]:-ls} -lha""#);
        } else {
            assert_eq!(line, " alias ls='ls -lha'");
        }
        // A leading space keeps it out of `HISTCONTROL=ignorespace` history.
        assert!(line.starts_with(' '));
    }

    #[test]
    fn nothing_is_typed_into_a_plain_shell() {
        assert!(ShellInit::default().lines().is_empty());
    }

    #[test]
    fn only_known_runtimes_and_plain_names_are_re_entered() {
        assert_eq!(
            enter_command(&container("dev-box.1", "distrobox")).as_deref(),
            Some("distrobox enter dev-box.1"),
        );
        // An unrecognised runtime: we have no idea what re-enters it.
        assert!(enter_command(&container("bottle", "docker")).is_none());
        // A name we'd have to quote is refused outright rather than escaped —
        // podman never issues one, so it can only be a mangled saved file.
        assert!(enter_command(&container("a; rm -rf ~", "toolbox")).is_none());
        assert!(enter_command(&container("", "toolbox")).is_none());
    }
}
