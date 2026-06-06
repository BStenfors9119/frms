//! Per-session state for the dockable plugin panel.
//!
//! The plugin panel hosts a small tab control across multiple plugins —
//! "DB", "Profile", "Notes", "Terminals" — each rendering its own body.
//! Visibility, dock side, and the active tab(s) are tracked here; each
//! plugin's actual data lives in its own module (DbPanel, Prefs, NotesState,
//! TerminalsState).
//!
//! The panel can be docked on either side of the window (left of the file
//! browser, or right of everything) and optionally split into two stacked
//! slots so two plugins are visible at once (e.g. Notes on top, Terminals
//! on the bottom).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginTab {
    Db,
    Profile,
    Notes,
    Terminals,
}

impl PluginTab {
    pub const ALL: &'static [PluginTab] = &[
        PluginTab::Db,
        PluginTab::Profile,
        PluginTab::Notes,
        PluginTab::Terminals,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PluginTab::Db        => "DB",
            PluginTab::Profile   => "Profile",
            PluginTab::Notes     => "Notes",
            PluginTab::Terminals => "Terminals",
        }
    }
}

/// Which edge of the window the panel docks against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockSide {
    /// Left of the file browser.
    Left,
    /// Right of everything (the default).
    Right,
}

/// One of the (up to two) stacked plugin views inside the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginSlot {
    Top,
    Bottom,
}

pub struct PluginPanel {
    pub visible:    bool,
    /// Which window edge the panel docks against.
    pub dock:       DockSide,
    /// Active tab for the top (or only) slot.
    pub active_tab: PluginTab,
    /// Active tab for the bottom slot — only shown when `split` is true.
    pub bottom_tab: PluginTab,
    /// When true the panel is split into two stacked slots.
    pub split:      bool,
    /// Height in logical pixels of the top slot when split; the bottom slot
    /// fills the remainder. User-adjustable via the divider drag handle.
    pub top_height: f32,
    /// Width in logical pixels; user-adjustable via the drag handle on the inner edge.
    pub width:      f32,
}

pub const DEFAULT_WIDTH:      f32 = 480.0;
pub const MIN_WIDTH:          f32 = 260.0;
pub const MAX_WIDTH:          f32 = 1600.0;
pub const DEFAULT_TOP_HEIGHT: f32 = 280.0;
pub const MIN_SLOT_HEIGHT:    f32 = 120.0;

impl Default for PluginPanel {
    fn default() -> Self {
        Self {
            visible:    false,
            dock:       DockSide::Right,
            active_tab: PluginTab::Db,
            bottom_tab: PluginTab::Terminals,
            split:      false,
            top_height: DEFAULT_TOP_HEIGHT,
            width:      DEFAULT_WIDTH,
        }
    }
}

impl PluginPanel {
    pub fn set_width(&mut self, w: f32) {
        self.width = w.clamp(MIN_WIDTH, MAX_WIDTH);
    }

    pub fn set_top_height(&mut self, h: f32) {
        // The bottom slot keeps at least MIN_SLOT_HEIGHT via Length::Fill, so
        // we only floor the top slot here; the overall panel height isn't known
        // in this context.
        self.top_height = h.max(MIN_SLOT_HEIGHT);
    }

    /// The active tab for a given slot.
    pub fn tab(&self, slot: PluginSlot) -> PluginTab {
        match slot {
            PluginSlot::Top    => self.active_tab,
            PluginSlot::Bottom => self.bottom_tab,
        }
    }

    /// Set the active tab for a given slot.
    pub fn set_tab(&mut self, slot: PluginSlot, tab: PluginTab) {
        match slot {
            PluginSlot::Top    => self.active_tab = tab,
            PluginSlot::Bottom => self.bottom_tab = tab,
        }
    }

    /// Toggle the dock side between the two window edges.
    pub fn toggle_dock(&mut self) {
        self.dock = match self.dock {
            DockSide::Left  => DockSide::Right,
            DockSide::Right => DockSide::Left,
        };
    }

    /// Toggle the split. When enabling, ensure the bottom slot shows a
    /// different plugin than the top so the split is immediately useful.
    pub fn toggle_split(&mut self) {
        self.split = !self.split;
        if self.split && self.bottom_tab == self.active_tab {
            self.bottom_tab = PluginTab::ALL
                .iter()
                .copied()
                .find(|t| *t != self.active_tab)
                .unwrap_or(PluginTab::Terminals);
        }
    }
}
