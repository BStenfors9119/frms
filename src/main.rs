mod agent;
mod app;
mod cdp;
mod chat_api;
mod clipboard;
mod db;
mod db_history;
mod db_panel;
mod splash_video;
mod claude_prompt;
mod stats;
mod editor;
mod file_browser;
mod fonts;
mod icon;
mod layout;
mod notes;
mod pane;
mod persisted;
mod plugin_panel;
mod port;
mod prefs;
mod receivers;
mod scp;
mod session;
mod telemetry;
mod terminal;
mod terminals;
mod theme;
mod ui;

use iced::{Size, window};

fn main() -> iced::Result {
    // `frms --export-icon <path>` — render the app icon as a PNG and exit.
    // Used by install.sh to generate the desktop icon from the binary.
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--export-icon") {
        let path = args.next().unwrap_or_else(|| "frms.png".into());
        match icon::export_png(std::path::Path::new(&path)) {
            Some(()) => {
                println!("wrote {path}");
                return Ok(());
            }
            None => {
                eprintln!("failed to write {path}");
                std::process::exit(1);
            }
        }
    }

    eprintln!("[frms] starting up");

    // Best-effort crash reporting, gated on the same opt-out as the rest of
    // telemetry — and only after the first-run notice has been acknowledged, so
    // nothing is sent before consent.
    let prefs = prefs::Prefs::load();
    telemetry::install_panic_hook(prefs.telemetry_notice_ack && prefs.telemetry);

    let mut window_settings = window::Settings {
        size:     Size::new(1400.0, 900.0),
        min_size: Some(Size::new(800.0, 500.0)),
        icon:     icon::build(),
        // The title-bar X is routed through `window_close_listener` →
        // `Message::Quit` → our hard shutdown, so disable iced's built-in
        // graceful exit (which hangs on Windows joining blocked PTY threads).
        exit_on_close_request: false,
        ..Default::default()
    };
    // Platform tweaks (e.g. the Linux Wayland/X11 app_id that binds the window
    // to frms.desktop) live in the port layer, not behind a cfg here.
    port::window::configure(&mut window_settings);

    iced::application(app::title, app::update, app::view)
        .theme(app::theme)
        .subscription(app::subscription)
        .scale_factor(app::scale_factor)
        .window(window_settings)
        .font(fonts::LIBERATION_SANS)
        .font(fonts::LIBERATION_SANS_BOLD)
        .font(fonts::LIBERATION_SANS_ITALIC)
        .font(fonts::LIBERATION_SANS_BOLD_ITALIC)
        .font(fonts::NOTO_SYMBOLS_2)
        .font(fonts::RESET_ARROW)
        .font(fonts::LIBERATION_MONO)
        .run_with(app::initialize)
}
