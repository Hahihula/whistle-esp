//! The 800x480 screen: a header bar, a status line, the transcript and a
//! timing footer. Only regions that change are redrawn: the frame buffer is
//! scanned out of PSRAM continuously, and big redraws compete with that
//! stream for PSRAM bandwidth (s31-rust-examples README).

use embedded_graphics::{
    mono_font::{MonoFont, MonoTextStyle, iso_8859_1, iso_8859_2},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Alignment, Baseline, Text},
};

use esp32_s31_korvo::display::{Canvas, HEIGHT, WIDTH};
use whistle_common::text;

pub const BG: Rgb565 = Rgb565::new(1, 3, 4);
pub const HEADER_BG: Rgb565 = Rgb565::new(2, 14, 6);
pub const ACCENT: Rgb565 = Rgb565::new(12, 52, 16);
pub const GREY: Rgb565 = Rgb565::new(18, 38, 18);
pub const WHITE: Rgb565 = Rgb565::WHITE;
pub const GREEN: Rgb565 = Rgb565::GREEN;
pub const BLUE: Rgb565 = Rgb565::new(10, 40, 31);
pub const AMBER: Rgb565 = Rgb565::new(31, 45, 0);
pub const RED: Rgb565 = Rgb565::RED;

const HEADER: Rectangle = Rectangle::new(Point::new(0, 0), Size::new(WIDTH as u32, 56));
const STATUS: Rectangle = Rectangle::new(Point::new(0, 70), Size::new(WIDTH as u32, 34));
const BODY: Rectangle = Rectangle::new(Point::new(0, 116), Size::new(WIDTH as u32, 284));
const HINT: Rectangle = Rectangle::new(Point::new(0, 402), Size::new(WIDTH as u32, 22));
const FOOTER: Rectangle = Rectangle::new(Point::new(0, 430), Size::new(WIDTH as u32, 50));
const MARGIN: i32 = 24;
const LINE_H: i32 = 28;

fn fill(c: &mut Canvas, r: Rectangle, color: Rgb565) {
    let _ = r.into_styled(PrimitiveStyle::with_fill(color)).draw(c);
}

fn text(c: &mut Canvas, s: &str, p: Point, style: MonoTextStyle<'_, Rgb565>, align: Alignment) {
    let _ = Text::with_alignment(s, p, style, align).draw(c);
}

/// The whole screen once, before the LCD stream starts.
pub fn draw_frame(c: &mut Canvas) {
    fill(
        c,
        Rectangle::new(Point::zero(), Size::new(WIDTH as u32, HEIGHT as u32)),
        BG,
    );
    fill(c, HEADER, HEADER_BG);
    let big = MonoTextStyle::new(&iso_8859_1::FONT_10X20, WHITE);
    let small = MonoTextStyle::new(&iso_8859_1::FONT_7X13, ACCENT);
    text(
        c,
        "Running Cactus Whistle by Cactus Compute",
        Point::new(MARGIN, 30),
        big,
        Alignment::Left,
    );
    text(
        c,
        "speech recognition on the ESP32-S31, no_std Rust, no network",
        Point::new(MARGIN, 47),
        small,
        Alignment::Left,
    );
    text(
        c,
        "huggingface.co/Cactus-Compute/whistle",
        Point::new(WIDTH as i32 - MARGIN, 47),
        MonoTextStyle::new(&iso_8859_1::FONT_7X13, GREY),
        Alignment::Right,
    );
}

pub fn status(c: &mut Canvas, s: &str, color: Rgb565) {
    fill(c, STATUS, BG);
    text(
        c,
        s,
        Point::new(MARGIN, 96),
        MonoTextStyle::new(&iso_8859_1::FONT_10X20, color),
        Alignment::Left,
    );
}

/// The transcript, word-wrapped. Latin-2 for Polish, Latin-1 otherwise.
pub fn body(c: &mut Canvas, s: &str, polish: bool) {
    fill(c, BODY, BG);
    let font: &MonoFont = if polish {
        &iso_8859_2::FONT_10X20
    } else {
        &iso_8859_1::FONT_10X20
    };
    let cols = ((WIDTH as i32 - 2 * MARGIN) / font.character_size.width as i32) as usize;
    let style = MonoTextStyle::new(font, WHITE);
    let max_lines = (BODY.size.height as i32 / LINE_H) as usize;
    for (i, line) in text::wrap(s, max_lines, |_| cols).iter().enumerate() {
        let _ = Text::with_baseline(
            line,
            Point::new(MARGIN, BODY.top_left.y + 4 + i as i32 * LINE_H),
            style,
            Baseline::Top,
        )
        .draw(c);
    }
}

/// A hint under the transcript.
pub fn hint(c: &mut Canvas, s: &str) {
    fill(c, HINT, BG);
    text(
        c,
        s,
        Point::new(MARGIN, 416),
        MonoTextStyle::new(&iso_8859_1::FONT_7X13, ACCENT),
        Alignment::Left,
    );
}

pub fn footer(c: &mut Canvas, line1: &str, line2: &str) {
    fill(c, FOOTER, BG);
    let style = MonoTextStyle::new(&iso_8859_1::FONT_7X13, GREY);
    text(c, line1, Point::new(MARGIN, 450), style, Alignment::Left);
    text(c, line2, Point::new(MARGIN, 468), style, Alignment::Left);
}
