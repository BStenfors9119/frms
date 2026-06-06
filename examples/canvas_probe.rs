//! Throwaway: render the terminal text the way ui/terminal.rs does, comparing
//! Liberation Mono / Font::MONOSPACE / default on a cell_w grid, under the
//! same application scale factor the IDE uses (Extra Large = 1.30). The app
//! screenshots itself via window::screenshot (GPU readback — no compositor
//! involvement) and writes /tmp/canvas_probe.png, then exits.
//!
//! Run:  cargo run --example canvas_probe

use iced::widget::canvas::{self, Canvas, Frame, Text};
use iced::{
    alignment, mouse, time, window, Color, Element, Font, Length, Pixels, Point, Rectangle,
    Size, Subscription, Task,
};
use std::time::Duration;

// Load the SAME three fonts in the SAME order the real app does, so this
// probe replicates the runtime as closely as possible.
static LIBERATION_SANS: &[u8] = include_bytes!("../assets/fonts/LiberationSans-Regular.ttf");
static NOTO_SYMBOLS_2:  &[u8] = include_bytes!("../assets/fonts/NotoSansSymbols2-Regular.ttf");
static LIBERATION_MONO: &[u8] = include_bytes!("../assets/fonts/LiberationMono-Regular.ttf");
const MONO_FONT: Font = Font::with_name("Liberation Mono");

fn main() -> iced::Result {
    iced::application("probe", update, view)
        .font(LIBERATION_SANS)
        .font(NOTO_SYMBOLS_2)
        .font(LIBERATION_MONO)
        .scale_factor(|_| 1.30) // matches the user's "Extra Large" pref
        // Pick a window size that survives the 1.30 scale factor without
        // overflow when the surface buffer is sized by monitor scale only.
        .window_size(iced::Size::new(700.0, 420.0))
        .subscription(subscription)
        .run()
}

#[derive(Default)]
struct State { shot_done: bool, ticks: u32 }

#[derive(Debug, Clone)]
enum Msg {
    Tick,
    Captured(window::Screenshot),
}

fn subscription(_: &State) -> Subscription<Msg> {
    time::every(Duration::from_millis(400)).map(|_| Msg::Tick)
}

fn update(state: &mut State, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Tick => {
            state.ticks += 1;
            // Wait a couple of frames for the canvas to actually paint.
            if state.ticks >= 2 && !state.shot_done {
                state.shot_done = true;
                return window::get_latest()
                    .and_then(window::screenshot)
                    .map(Msg::Captured);
            }
            Task::none()
        }
        Msg::Captured(shot) => {
            let w = shot.size.width;
            let h = shot.size.height;
            eprintln!("screenshot: {}x{} bytes={} scale={}", w, h, shot.bytes.len(), shot.scale_factor);
            let buf = image::RgbaImage::from_raw(w, h, shot.bytes.to_vec())
                .expect("rgba buf size mismatch");
            buf.save("/tmp/canvas_probe.png").unwrap();
            eprintln!("wrote /tmp/canvas_probe.png");
            std::process::exit(0);
        }
    }
}

fn view(_state: &State) -> Element<'_, Msg> {
    Canvas::new(Probe).width(Length::Fill).height(Length::Fill).into()
}

struct Probe;

impl canvas::Program<Msg> for Probe {
    type State = ();
    fn draw(&self, _: &(), r: &iced::Renderer, _: &iced::Theme, b: Rectangle, _: mouse::Cursor) -> Vec<canvas::Geometry> {
        let mut frame = Frame::new(r, b.size());
        frame.fill_rectangle(Point::ORIGIN, b.size(), Color::from_rgb8(28, 28, 28));

        // Exact ui/terminal.rs geometry at TerminalFontScale::Medium (no magnification),
        // PLUS a 3x magnified row so small-size rasterization is visible side by side.
        let sample = "Mango illi WWmm 0O claude code session";
        let fonts: [(&str, Font, f32); 6] = [
            ("Liberation Mono  (real size)", MONO_FONT, 1.0),
            ("Font::MONOSPACE  (real size)", Font::MONOSPACE, 1.0),
            ("Font::DEFAULT    (real size)", Font::DEFAULT, 1.0),
            ("Liberation Mono  (3x)",        MONO_FONT, 3.0),
            ("Font::MONOSPACE  (3x)",        Font::MONOSPACE, 3.0),
            ("Font::DEFAULT    (3x)",        Font::DEFAULT, 3.0),
        ];

        let mut y = 10.0;
        for (label, font, mag) in fonts {
            let cell_h = 17.0 * mag;
            let font_size = cell_h * 0.80;
            let cell_w = cell_h * 0.80 * 0.6;
            // cell gridlines (thin vertical bars at each cell boundary)
            for i in 0..=sample.len() as u32 {
                let x = i as f32 * cell_w;
                frame.fill_rectangle(
                    Point::new(x, y),
                    Size::new(1.0, cell_h),
                    Color::from_rgba8(80, 80, 130, 0.9),
                );
            }
            for (col, ch) in sample.chars().enumerate() {
                if ch == ' ' { continue; }
                frame.fill_text(Text {
                    content: ch.to_string(),
                    position: Point::new(col as f32 * cell_w, y),
                    size: Pixels(font_size),
                    color: Color::from_rgb8(220, 220, 220),
                    font,
                    horizontal_alignment: alignment::Horizontal::Left,
                    vertical_alignment: alignment::Vertical::Top,
                    line_height: iced::widget::text::LineHeight::Absolute(Pixels(cell_h)),
                    shaping: iced::widget::text::Shaping::Basic,
                });
            }
            // small label to the right
            frame.fill_text(Text {
                content: format!("  <- {label}"),
                position: Point::new(sample.len() as f32 * cell_w + 8.0, y + cell_h * 0.2),
                size: Pixels(14.0),
                color: Color::from_rgb8(160, 200, 160),
                font: Font::DEFAULT,
                horizontal_alignment: alignment::Horizontal::Left,
                vertical_alignment: alignment::Vertical::Top,
                line_height: iced::widget::text::LineHeight::Relative(1.2),
                shaping: iced::widget::text::Shaping::Basic,
            });
            y += cell_h + 12.0;
        }
        vec![frame.into_geometry()]
    }
}
