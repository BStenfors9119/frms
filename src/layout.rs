use iced::widget::pane_grid::{self, Configuration};

use crate::pane::PaneKind;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayoutKind {
    /// File browser (15%) + Editor area (85%). The editor area is itself
    /// a tabbed surface that holds Editor / Database / Claude N tabs.
    Terminal,
    /// Full-width Browser pane on top — no bottom row.
    Browser,
}

pub struct Layout {
    #[allow(dead_code)]
    pub kind:  LayoutKind,
    pub panes: pane_grid::State<PaneKind>,
}

impl Layout {
    pub fn new_terminal() -> Self {
        Self {
            kind:  LayoutKind::Terminal,
            panes: pane_grid::State::with_configuration(terminal_config()),
        }
    }

    pub fn new_browser() -> Self {
        Self {
            kind:  LayoutKind::Browser,
            panes: pane_grid::State::with_configuration(browser_config()),
        }
    }
}

// ── configurations ────────────────────────────────────────────────────────────

fn terminal_config() -> Configuration<PaneKind> {
    Configuration::Split {
        axis:  pane_grid::Axis::Vertical,
        ratio: 0.15,
        a: Box::new(Configuration::Pane(PaneKind::FileBrowser)),
        b: Box::new(Configuration::Pane(PaneKind::Editor)),
    }
}

fn browser_config() -> Configuration<PaneKind> {
    Configuration::Pane(PaneKind::Browser)
}
