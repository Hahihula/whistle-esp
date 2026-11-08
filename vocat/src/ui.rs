//! The round 360x360 screen: a status line, the transcript word-wrapped to
//! the circle, a hint and a timing footer.

use alloc::{format, string::String};

use embedded_graphics::{
    mono_font::{MonoFont, MonoTextStyle, iso_8859_1, iso_8859_2},
    pixelcolor::Rgb565,
    prelude::*,
    text::{Alignment, Text},
};

use esp_vocat::display::FrameBuffer;

use whistle_common::{inference::Transcript, text};

pub const GREY: Rgb565 = Rgb565::new(20, 40, 20);
pub const GREEN: Rgb565 = Rgb565::GREEN;
pub const BLUE: Rgb565 = Rgb565::new(10, 40, 31);
pub const AMBER: Rgb565 = Rgb565::new(31, 45, 0);
pub const RED: Rgb565 = Rgb565::RED;
/// Header colour, a light green.
pub const ACCENT: Rgb565 = Rgb565::new(12, 52, 16);

const R: i32 = 180;
const LINE_H: i32 = 24;
const BODY_TOP: i32 = 110;
const BODY_LINES: usize = 7;

pub struct Screen {
    status: String,
    status_color: Rgb565,
    body: String,
    polish: bool,
    hint: String,
    footer: String,
}

impl Screen {
    pub fn new() -> Self {
        Screen {
            status: String::new(),
            status_color: GREY,
            body: String::new(),
            polish: false,
            hint: String::new(),
            footer: String::new(),
        }
    }

    pub fn status(&mut self, s: &str, color: Rgb565) {
        self.status = String::from(s);
        self.status_color = color;
    }

    pub fn body(&mut self, s: &str) {
        self.body = String::from(s);
        self.polish = false;
        self.footer.clear();
    }

    pub fn hint(&mut self, s: &str) {
        self.hint = String::from(s);
    }

    pub fn transcript(&mut self, t: &Transcript) {
        self.status(if t.selftest { "Self-test" } else { "Heard" }, if t.selftest { GREY } else { GREEN });
        self.body = if t.text.is_empty() { String::from("(nothing)") } else { t.text.clone() };
        self.polish = t.lang == "pl";
        let mode = if crate::SPEC == cactus_whistle::Spec::Guide { "5-beam" } else { "fast/greedy" };
        self.footer = format!(
            "{} | {:.1}s audio | {:.1}s | {mode}",
            t.lang,
            t.audio_ms as f32 / 1000.0,
            (t.enc_ms + t.dec_ms) as f32 / 1000.0
        );
    }

    pub fn draw(&self, fb: &mut FrameBuffer) {
        let _ = fb.clear(Rgb565::BLACK);
        let big: &MonoFont = if self.polish { &iso_8859_2::FONT_10X20 } else { &iso_8859_1::FONT_10X20 };
        let small = &iso_8859_1::FONT_7X13;
        // Header, where the circle is still ~240 px wide.
        center(fb, "Cactus Whistle", 38, MonoTextStyle::new(small, ACCENT));
        center(fb, "by Cactus Compute", 53, MonoTextStyle::new(small, GREY));
        center(fb, &self.status, 84, MonoTextStyle::new(big, self.status_color));
        let char_w = big.character_size.width as i32;
        let lines = text::wrap(&self.body, BODY_LINES, |i| line_chars(i, char_w));
        let style = MonoTextStyle::new(big, Rgb565::WHITE);
        for (i, l) in lines.iter().enumerate() {
            center(fb, l, BODY_TOP + 18 + i as i32 * LINE_H, style);
        }
        center(fb, &self.hint, 290, MonoTextStyle::new(small, GREY));
        center(fb, &self.footer, 310, MonoTextStyle::new(small, GREY));
    }
}

fn center(fb: &mut FrameBuffer, s: &str, y: i32, style: MonoTextStyle<'_, Rgb565>) {
    if !s.is_empty() {
        let _ = Text::with_alignment(s, Point::new(R, y), style, Alignment::Center).draw(fb);
    }
}

/// Characters that fit on body line `i`: the chord of the circle at that
/// line's height, less a margin.
fn line_chars(i: usize, char_w: i32) -> usize {
    let y = BODY_TOP + i as i32 * LINE_H + LINE_H / 2 - R;
    let worst = y.abs() + LINE_H / 2;
    let half = libm::sqrtf((R * R - worst * worst).max(0) as f32) as i32;
    ((2 * half - 24) / char_w).max(4) as usize
}
