use super::Sys;
use crate::port::api::Window;

impl Window for Sys {
    fn configure(_settings: &mut iced::window::Settings) {
        // Nothing Windows-specific to apply to the window today.
    }
}
