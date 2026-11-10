//! The 1024x600 screen: a header bar, a status line, the transcript, a hint
//! and a timing footer.
//!
//! The display rotates three frame buffers (esp32-p4-function-ev
//! `display::FRAME_BUFFERS`), so the back buffer holds a frame from three
//! commits ago. Rather than track what each buffer is missing, [`Screen`]
//! keeps the whole state and redraws everything on every update; updates
//! are rare (a handful per transcription).

use alloc::string::String;

use embedded_graphics::{
    mono_font::{MonoFont, MonoTextStyle, iso_8859_1, iso_8859_2},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Alignment, Baseline, Text},
};

use esp32_p4_function_ev::display::{Display, WIDTH};
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

const HEADER: Rectangle = Rectangle::new(Point::new(0, 0), Size::new(WIDTH, 64));
const BODY: Rectangle = Rectangle::new(Point::new(0, 136), Size::new(WIDTH, 336));
const MARGIN: i32 = 32;
const LINE_H: i32 = 28;

/// Everything on screen.
pub struct Screen {
    status: (String, Rgb565),
    body: String,
    polish: bool,
    hint: String,
    footer: (String, String),
}

impl Screen {
    pub fn new() -> Self {
        Self {
            status: (String::new(), GREY),
            body: String::new(),
            polish: false,
            hint: String::new(),
            footer: (String::new(), String::new()),
        }
    }

    pub fn status(&mut self, s: &str, color: Rgb565) {
        self.status = (String::from(s), color);
    }

    /// The transcript. Latin-2 for Polish, Latin-1 otherwise.
    pub fn body(&mut self, s: &str, polish: bool) {
        self.body = String::from(s);
        self.polish = polish;
    }

    pub fn hint(&mut self, s: &str) {
        self.hint = String::from(s);
    }

    pub fn footer(&mut self, line1: &str, line2: &str) {
        self.footer = (String::from(line1), String::from(line2));
    }

    /// Draws the whole screen into the back buffer and shows it.
    pub async fn show(&self, lcd: &mut Display<'_>) {
        {
            let mut fb = lcd.back_buffer();
            clear(fb.as_bytes_mut(), BG);
            self.draw(&mut fb);
        }
        lcd.wait_for_vsync().await;
        lcd.commit();
    }

    /// Everything but the background (see [`clear`]).
    fn draw<D: DrawTarget<Color = Rgb565>>(&self, c: &mut D) {
        let _ = HEADER
            .into_styled(PrimitiveStyle::with_fill(HEADER_BG))
            .draw(c);
        let big = MonoTextStyle::new(&iso_8859_1::FONT_10X20, WHITE);
        let small = MonoTextStyle::new(&iso_8859_1::FONT_7X13, ACCENT);
        let grey = MonoTextStyle::new(&iso_8859_1::FONT_7X13, GREY);
        let text_at = |c: &mut D, s: &str, p: Point, st, align| {
            let _ = Text::with_alignment(s, p, st, align).draw(c);
        };
        text_at(
            c,
            "Running Cactus Whistle by Cactus Compute",
            Point::new(MARGIN, 34),
            big,
            Alignment::Left,
        );
        text_at(
            c,
            "speech recognition on the ESP32-P4, no_std Rust, no network",
            Point::new(MARGIN, 54),
            small,
            Alignment::Left,
        );
        text_at(
            c,
            "huggingface.co/Cactus-Compute/whistle",
            Point::new(WIDTH as i32 - MARGIN, 54),
            grey,
            Alignment::Right,
        );

        text_at(
            c,
            &self.status.0,
            Point::new(MARGIN, 110),
            MonoTextStyle::new(&iso_8859_1::FONT_10X20, self.status.1),
            Alignment::Left,
        );

        let font: &MonoFont = if self.polish {
            &iso_8859_2::FONT_10X20
        } else {
            &iso_8859_1::FONT_10X20
        };
        let cols = ((WIDTH as i32 - 2 * MARGIN) / font.character_size.width as i32) as usize;
        let style = MonoTextStyle::new(font, WHITE);
        let max_lines = (BODY.size.height as i32 / LINE_H) as usize;
        for (i, line) in text::wrap(&self.body, max_lines, |_| cols)
            .iter()
            .enumerate()
        {
            let _ = Text::with_baseline(
                line,
                Point::new(MARGIN, BODY.top_left.y + 4 + i as i32 * LINE_H),
                style,
                Baseline::Top,
            )
            .draw(c);
        }

        text_at(
            c,
            &self.hint,
            Point::new(MARGIN, 496),
            small,
            Alignment::Left,
        );
        text_at(
            c,
            &self.footer.0,
            Point::new(MARGIN, 550),
            grey,
            Alignment::Left,
        );
        text_at(
            c,
            &self.footer.1,
            Point::new(MARGIN, 570),
            grey,
            Alignment::Left,
        );
    }
}

/// Fills a frame with one colour, a word (two pixels) at a time: ~16 ms for
/// the 1.2 MB frame in PSRAM (~330 ms while core 1 copies the output head
/// into PSRAM at boot).
fn clear(frame: &mut [u8], color: Rgb565) {
    let px = embedded_graphics::pixelcolor::raw::RawU16::from(color).into_inner();
    let word = u32::from(px) * 0x0001_0001;
    // SAFETY: any bit pattern is a valid u32; the frame buffer is 64-byte
    // aligned, so the head and tail are empty, but are handled anyway.
    let (head, words, tail) = unsafe { frame.align_to_mut::<u32>() };
    words.fill(word.to_le());
    let bytes = px.to_le_bytes();
    for (i, b) in head.iter_mut().chain(tail.iter_mut()).enumerate() {
        *b = bytes[i % 2];
    }
}
