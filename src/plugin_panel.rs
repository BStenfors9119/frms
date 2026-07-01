//! Per-session state for the dockable plugin panel.
//!
//! The plugin panel hosts a small tab control across multiple plugins —
//! "Profile", "Notes", "Terminals" — each rendering its own body.
//! Visibility, dock side, and the active tab(s) are tracked here; each
//! plugin's actual data lives in its own module (Prefs, NotesState,
//! TerminalsState). The database tooling is not a plugin — it lives in the
//! per-project Database center tab.
//!
//! The panel can be docked on either side of the window (left of the file
//! browser, or right of everything) and optionally split into two stacked
//! slots so two plugins are visible at once (e.g. Notes on top, Terminals
//! on the bottom).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginTab {
    Profile,
    Notes,
    Terminals,
    Receivers,
}

impl PluginTab {
    pub const ALL: &'static [PluginTab] = &[
        PluginTab::Profile,
        PluginTab::Notes,
        PluginTab::Terminals,
        PluginTab::Receivers,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PluginTab::Profile   => "Profile",
            PluginTab::Notes     => "Notes",
            PluginTab::Terminals => "Terminals",
            // The "Receivers" inventory is exposed under the generic "SSH" name:
            // it started as TSR-receiver-specific but will host other SSH
            // devices too. The enum/module keep the `Receivers` name.
            PluginTab::Receivers => "SSH",
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

#[derive(Clone, Debug)]
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
    /// Plugins the user has hidden from the tab bar via the per-tab `✕`. A
    /// hidden plugin keeps its state but isn't shown as a tab until re-added
    /// through the `+` picker. Empty by default — every plugin visible — which
    /// also matches how an old persisted file (predating this field) restores.
    pub hidden:     Vec<PluginTab>,
    /// Transient: the `+` add-plugin picker is open. Not persisted.
    pub adding:     bool,
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
            active_tab: PluginTab::Profile,
            bottom_tab: PluginTab::Terminals,
            split:      false,
            top_height: DEFAULT_TOP_HEIGHT,
            width:      DEFAULT_WIDTH,
            hidden:     Vec::new(),
            adding:     false,
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

    /// Whether `tab` is currently shown as a tab (not hidden by the user).
    pub fn is_shown(&self, tab: PluginTab) -> bool {
        !self.hidden.contains(&tab)
    }

    /// Plugins currently shown as tabs, in the canonical [`PluginTab::ALL`] order.
    pub fn shown_tabs(&self) -> impl Iterator<Item = PluginTab> + '_ {
        PluginTab::ALL.iter().copied().filter(move |t| self.is_shown(*t))
    }

    /// Plugins the user has hidden, in canonical order — the `+` picker's contents.
    pub fn hidden_tabs(&self) -> impl Iterator<Item = PluginTab> + '_ {
        PluginTab::ALL.iter().copied().filter(move |t| !self.is_shown(*t))
    }

    /// How many plugins are shown as tabs.
    pub fn shown_count(&self) -> usize {
        PluginTab::ALL.len() - self.hidden.len()
    }

    /// Hide `tab` from the tab bar. Refuses to hide the last shown plugin so
    /// the tab strip never goes empty. If the hidden plugin was a slot's active
    /// tab, that slot falls back to the first still-shown plugin.
    pub fn hide_tab(&mut self, tab: PluginTab) {
        if self.shown_count() <= 1 || self.hidden.contains(&tab) {
            return;
        }
        self.hidden.push(tab);
        let fallback = self.shown_tabs().next().unwrap_or(tab);
        if self.active_tab == tab { self.active_tab = fallback; }
        if self.bottom_tab == tab { self.bottom_tab = fallback; }
    }

    /// Re-show a previously hidden plugin.
    pub fn show_tab(&mut self, tab: PluginTab) {
        self.hidden.retain(|t| *t != tab);
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
            // Prefer a still-shown plugin so the freshly split bottom slot lands
            // on a usable tab rather than a hidden one.
            let active = self.active_tab;
            let bottom = self.shown_tabs()
                .find(|t| *t != active)
                .unwrap_or(active);
            self.bottom_tab = bottom;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiding_active_tab_falls_back_to_first_shown() {
        let mut p = PluginPanel::default(); // active = Profile, all shown
        p.hide_tab(PluginTab::Profile);
        assert!(!p.is_shown(PluginTab::Profile));
        // Active moved off the now-hidden plugin to the first still-shown one.
        assert_eq!(p.active_tab, PluginTab::Notes);
        assert_eq!(p.shown_count(), 3);
    }

    #[test]
    fn last_shown_plugin_cannot_be_hidden() {
        let mut p = PluginPanel::default();
        for t in [PluginTab::Notes, PluginTab::Terminals, PluginTab::Receivers] {
            p.hide_tab(t);
        }
        assert_eq!(p.shown_count(), 1);
        // The lone remaining plugin refuses to hide.
        p.hide_tab(PluginTab::Profile);
        assert_eq!(p.shown_count(), 1);
        assert!(p.is_shown(PluginTab::Profile));
    }

    #[test]
    fn show_tab_restores_a_hidden_plugin() {
        let mut p = PluginPanel::default();
        p.hide_tab(PluginTab::Terminals);
        assert!(!p.is_shown(PluginTab::Terminals));
        p.show_tab(PluginTab::Terminals);
        assert!(p.is_shown(PluginTab::Terminals));
        assert_eq!(p.shown_count(), PluginTab::ALL.len());
    }
}
