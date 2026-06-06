use iced::widget::{button, container, text, tooltip};
use iced::{Element, Renderer, Theme};

use crate::fonts::UI_FONT;

pub mod browser;
pub mod buttons;
pub mod confirm_dialog;
pub mod db_panel;
pub mod editor;
pub mod prompt_response;
pub mod file_browser;
pub mod header;
pub mod new_session_dialog;
pub mod pane;
pub mod plugin_panel;
pub mod prefs_dialog;
pub mod preview;
pub mod session_bar;
pub mod splash;
pub mod terminal;

/// Wrap a small icon button in a hover tooltip — the single-glyph buttons
/// are tiny, so spell out what each one does.
pub fn with_tip<'a, M: Clone + 'a>(
    btn: button::Button<'a, M, Theme, Renderer>,
    tip: &'a str,
) -> Element<'a, M, Theme, Renderer> {
    tooltip(btn, text(tip).font(UI_FONT).size(11), tooltip::Position::Bottom)
        .gap(4)
        .padding(6)
        .style(container::rounded_box)
        .into()
}
