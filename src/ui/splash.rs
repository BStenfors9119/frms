use iced::font::{Family, Weight};
use iced::widget::{button, column, container, image, text};
use iced::{Alignment, Background, Border, Color, Element, Font, Length, Theme};

use crate::app::{Message, SplashAnim};
use crate::splash_video;

// ── terminal-style font ───────────────────────────────────────────────────────

const TERMINAL: Font = Font {
    family:  Family::Monospace,
    weight:  Weight::Bold,
    style:   iced::font::Style::Normal,
    stretch: iced::font::Stretch::Normal,
};

// ── public view ───────────────────────────────────────────────────────────────

pub fn view<'a>(
    frame: Option<&'a Vec<u8>>,
    anim:  &SplashAnim,
) -> Element<'a, Message> {
    match anim {
        SplashAnim::Video => {
            let video_area: Element<'a, Message> = match frame {
                Some(jpeg) => {
                    let handle = image::Handle::from_bytes(jpeg.clone());
                    image(handle)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .content_fit(iced::ContentFit::Contain)
                        .into()
                }
                None => container(
                    text("Loading…")
                        .size(14)
                        .color(dim()),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into(),
            };

            container(video_area)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(black_bg)
                .into()
        }

        SplashAnim::FadeOut(t) => {
            let handle = frame.and_then(|jpeg| {
                splash_video::blend_to_black(jpeg, *t).map(|(w, h, px)| {
                    image::Handle::from_rgba(w, h, px)
                })
            });

            let inner: Element<'a, Message> = match handle {
                Some(h) => image(h)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .content_fit(iced::ContentFit::Contain)
                    .into(),
                None => container(iced::widget::Space::new(Length::Fill, Length::Fill))
                    .into(),
            };

            container(inner)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(black_bg)
                .into()
        }

        SplashAnim::FadeIn(t) => {
            container(splash_content(*t))
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(black_bg)
                .into()
        }

        SplashAnim::Text => {
            let btn = button(
                text("Get Started")
                    .font(TERMINAL)
                    .size(16)
                    .color(Color::WHITE),
            )
            .on_press(Message::SplashDismissed)
            .padding([10, 28])
            .style(start_button_style);

            container(
                column![splash_content(1.0), btn]
                    .spacing(28)
                    .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(black_bg)
            .into()
        }
    }
}

// ── splash content: "frms" header + tagline ───────────────────────────────────

fn splash_content(alpha: f32) -> Element<'static, Message> {
    let a = alpha.clamp(0.0, 1.0);

    let header: Element<'static, Message> = match crate::icon::image_handle() {
        Some(handle) => image(handle)
            .width(Length::Fixed(220.0))
            .height(Length::Fixed(220.0))
            .into(),
        // Font load shouldn't fail, but fall back to text just in case.
        None => text("frms")
            .font(TERMINAL)
            .size(64)
            .color(Color { r: 1.0, g: 1.0, b: 1.0, a })
            .into(),
    };

    let tagline = text("Make something amazing now...")
        .font(TERMINAL)
        .size(24)
        .color(Color { r: 0.80, g: 0.75, b: 1.0, a });

    column![header, tagline]
        .spacing(12)
        .align_x(Alignment::Center)
        .into()
}

fn dim() -> Color {
    Color { r: 0.4, g: 0.38, b: 0.45, a: 1.0 }
}

// ── styles ────────────────────────────────────────────────────────────────────

fn black_bg(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::BLACK)),
        ..Default::default()
    }
}

fn start_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => Color::from_rgb8(100, 65, 200),
        _                       => Color::from_rgb8(70,  40, 160),
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: Color::WHITE,
        border:     Border { radius: 4.0.into(), ..Default::default() },
        ..Default::default()
    }
}
