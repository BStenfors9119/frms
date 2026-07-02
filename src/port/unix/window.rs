use super::Sys;
use crate::port::api::Window;

impl Window for Sys {
    fn configure(settings: &mut iced::window::Settings) {
        // Wayland app_id / X11 WM_CLASS — must match the .desktop filename
        // (frms.desktop) so GNOME associates the window with the pinned
        // launcher. The `application_id` field only exists on Linux/BSD builds
        // of iced, so gate it here (this `cfg` is quarantined inside the port
        // layer, where finer-grained unix distinctions are allowed).
        #[cfg(target_os = "linux")]
        {
            settings.platform_specific.application_id = "frms".into();
        }
        let _ = settings;
    }
}
