//! The boot splash: the Espressif mark, a heart and the Cactus Compute mark
//! in one line in the middle of the screen. The line fades in, the two marks
//! move apart vertically, and the names "espressif" and "cactuscompute"
//! appear next to them: espressif loves cactuscompute.
//!
//! The marks are white line art, so the firmware only needs their coverage
//! (one byte per pixel, `tools/make_splash_assets.py` writes them). On the
//! black background the coverage is also the brightness, which is how the
//! logos fade in.
//!
//! Only the areas the last frame drew are erased and redrawn, so a frame
//! touches the rows the animation is on instead of the whole screen (the
//! Korvo's LCD is scanned out of PSRAM, and big redraws compete with that
//! stream).

use core::time::Duration;

use embedded_graphics::{
    mono_font::{MonoFont, MonoTextStyle, ascii},
    pixelcolor::{Rgb565, RgbColor},
    prelude::*,
    primitives::Rectangle,
    text::{Baseline, Text},
};

/// How long the whole splash lasts.
pub const DURATION: Duration = Duration::from_millis(2600);
/// The line fades in over this long.
const FADE_IN_MS: f32 = 400.0;
/// The marks travel apart between these two moments.
const SPLIT_FROM_MS: f32 = 450.0;
const SPLIT_TO_MS: f32 = 1500.0;
/// The names fade in (and slide in) between these two moments.
const NAME_FROM_MS: f32 = 1400.0;
const NAME_TO_MS: f32 = 2100.0;
/// How far a name slides in, in pixels.
const NAME_SLIDE: i32 = 12;

/// The name of the Espressif mark.
pub const ESPRESSIF_NAME: &str = "espressif";
/// The name of the Cactus Compute mark.
pub const CACTUS_NAME: &str = "cactuscompute";

/// A mark: the coverage of every pixel, 0 (nothing) to 255 (full white).
///
/// The masks are written by `tools/make_splash_assets.py`: a u16 width, a
/// u16 height and then one byte per pixel, row by row.
pub struct Mask {
    width: u16,
    height: u16,
    data: &'static [u8],
}

impl Mask {
    /// Reads a mask written by `tools/make_splash_assets.py`.
    ///
    /// # Panics
    ///
    /// If the mask is truncated or its header does not match its data.
    #[must_use]
    pub fn new(bytes: &'static [u8]) -> Self {
        assert!(bytes.len() >= 4, "splash mask: no header");
        let width = u16::from_le_bytes([bytes[0], bytes[1]]);
        let height = u16::from_le_bytes([bytes[2], bytes[3]]);
        let data = &bytes[4..];
        assert_eq!(
            data.len(),
            usize::from(width) * usize::from(height),
            "splash mask: size does not match its header"
        );
        Mask { width, height, data }
    }

    /// The width of the mark in pixels.
    #[must_use]
    pub fn width(&self) -> i32 {
        i32::from(self.width)
    }

    /// The height of the mark in pixels.
    #[must_use]
    pub fn height(&self) -> i32 {
        i32::from(self.height)
    }
}

/// How the splash is laid out on a screen.
pub struct Config {
    /// The screen size in pixels.
    pub width: i32,
    pub height: i32,
    /// The font for the first name.
    pub font: &'static MonoFont<'static>,
    /// The font for the second name. The names have a different number of
    /// letters, so they get a different width to keep the composition
    /// centred.
    pub font_b: &'static MonoFont<'static>,
    /// How big the names are drawn, in times the font's own size.
    pub name_scale: u32,
    /// How far the marks travel apart, in pixels.
    pub travel: i32,
    /// The space between a mark and its name, in pixels.
    pub name_gap: i32,
    /// The colour of the Espressif mark.
    pub espressif_color: (u8, u8, u8),
    /// The colour of the Cactus Compute mark.
    pub cactus_color: (u8, u8, u8),
    /// The colour of the heart.
    pub heart_color: (u8, u8, u8),
}

/// The splash, ready to be drawn frame by frame.
pub struct Splash {
    espressif: Mask,
    cactus: Mask,
    heart: Mask,
    cfg: Config,
    /// The areas the last frame drew, erased before the next one.
    painted: [Rectangle; 5],
    painted_len: usize,
    cleared: bool,
    /// What has settled and stays on the screen without being drawn again.
    ///
    /// On a display that is scanned out of memory (the Korvo's LCD) every
    /// frame that is written has to fit into the blanking of the panel, or
    /// the scan-out falls behind and the picture drifts. So what does not
    /// move any more is drawn once and then left alone.
    settled: Settled,
}

/// The parts of the splash that have come to a stand-still.
#[derive(Clone, Copy)]
struct Settled {
    heart: bool,
    marks: bool,
    names: bool,
}

impl Splash {
    /// The splash on the round 360x360 screen of the ESP-VoCat.
    ///
    /// The marks are small and the names are small too; the two rows mirror
    /// each other around the heart.
    #[must_use]
    pub fn vocat() -> Self {
        Splash::new(
            Mask::new(include_bytes!("../assets/vocat/splash_espressif.msk")),
            Mask::new(include_bytes!("../assets/vocat/splash_cactus.msk")),
            Mask::new(include_bytes!("../assets/vocat/splash_heart.msk")),
            Config {
                width: 360,
                height: 360,
                font: &ascii::FONT_10X20,
                font_b: &ascii::FONT_6X13,
                name_scale: 2,
                travel: 62,
                name_gap: 8,
                espressif_color: (235, 24, 32),
                cactus_color: (255, 255, 255),
                heart_color: (255, 64, 96),
            },
        )
    }

    /// The splash on the 800x480 screen of the ESP32-S31-Korvo-1.
    ///
    /// The names are in a 10x20 and a 6x13 font, so that the two rows are
    /// the same width and mirror each other around the heart.
    #[must_use]
    pub fn korvo() -> Self {
        Splash::new(
            Mask::new(include_bytes!("../assets/korvo/splash_espressif.msk")),
            Mask::new(include_bytes!("../assets/korvo/splash_cactus.msk")),
            Mask::new(include_bytes!("../assets/korvo/splash_heart.msk")),
            Config {
                width: 800,
                height: 480,
                font: &ascii::FONT_10X20,
                font_b: &ascii::FONT_6X13,
                name_scale: 2,
                travel: 76,
                name_gap: 16,
                espressif_color: (235, 24, 32),
                cactus_color: (255, 255, 255),
                heart_color: (255, 64, 96),
            },
        )
    }

    /// A splash with the three marks and the layout of a screen.
    #[must_use]
    pub fn new(espressif: Mask, cactus: Mask, heart: Mask, cfg: Config) -> Self {
        Splash {
            espressif,
            cactus,
            heart,
            cfg,
            painted: [Rectangle::zero(); 5],
            painted_len: 0,
            cleared: false,
            settled: Settled {
                heart: false,
                marks: false,
                names: false,
            },
        }
    }

    /// Says that the screen is already black, so that the first frame does
    /// not have to clear it. Clearing the whole screen is a lot of writing
    /// for a display that is scanned out of memory (the Korvo's LCD).
    pub fn assume_cleared(&mut self) {
        self.cleared = true;
    }

    /// Draws the frame at `t` since the start of the splash.
    pub fn draw<D: DrawTarget<Color = Rgb565>>(&mut self, target: &mut D, t: Duration) {
        if !self.cleared {
            let _ = target.clear(Rgb565::BLACK);
            self.cleared = true;
        }
        // Erase what the last frame drew.
        for rect in self.painted[..self.painted_len].iter() {
            let _ = target.fill_solid(rect, Rgb565::BLACK);
        }
        self.painted_len = 0;

        let ms = t.as_millis() as f32;
        let fade = ramp(ms, 0.0, FADE_IN_MS);
        let split = ramp(ms, SPLIT_FROM_MS, SPLIT_TO_MS);
        let name = ramp(ms, NAME_FROM_MS, NAME_TO_MS);

        // The layout, copied out of the config so that the frame can be
        // drawn while the areas it touches are remembered.
        let width = self.cfg.width;
        let height = self.cfg.height;
        let font = self.cfg.font;
        let font_b = self.cfg.font_b;
        let name_scale = self.cfg.name_scale;
        let travel_max = self.cfg.travel;
        let name_gap = self.cfg.name_gap;
        let espressif_color = self.cfg.espressif_color;
        let cactus_color = self.cfg.cactus_color;
        let heart_color = self.cfg.heart_color;

        let cx = width / 2;
        let cy = height / 2;

        let (ew, eh) = (self.espressif.width(), self.espressif.height());
        let (cw, ch) = (self.cactus.width(), self.cactus.height());
        let (hw, hh) = (self.heart.width(), self.heart.height());

        // The two rows: the Espressif row is the mark, a gap and the name; the
        // Cactus Compute row is the name, a gap and the mark. The rows mirror
        // each other, so they have the same width: the gap of the narrower
        // row makes up for the difference between the marks and the names.
        // Both rows are centred on the screen, so the whole composition is.
        let scale = name_scale.max(1);
        let esp_name_h = font.character_size.height as i32 * scale as i32;
        let cat_name_h = font_b.character_size.height as i32 * scale as i32;
        let esp_name_w = ESPRESSIF_NAME.len() as i32 * font.character_size.width as i32 * scale as i32;
        let cat_name_w = CACTUS_NAME.len() as i32 * font_b.character_size.width as i32 * scale as i32;

        let esp_row = ew + esp_name_w;
        let cat_row = cw + cat_name_w;
        let row_width = esp_row.max(cat_row) + name_gap;
        let esp_gap = row_width - esp_row;
        let cat_gap = row_width - cat_row;

        // The horizontal places never change: only the vertical ones do, so
        // the marks travel straight up and down.
        let esp_x = cx - row_width / 2;
        let cat_x = cx + row_width / 2 - cw;
        let heart_x = cx - hw / 2;

        // Vertically the marks start on the centre line and move to their
        // rows, above and below the heart.
        let travel = libm::roundf(travel_max as f32 * split) as i32;
        let esp_y = cy - eh / 2 - travel;
        let cat_y = cy - ch / 2 + travel;
        let heart_y = cy - hh / 2;

        // The names are centred on their marks, in the rows the marks end up
        // in, and slide in from the outside.
        let slide = libm::roundf(NAME_SLIDE as f32 * (1.0 - name)) as i32;
        let esp_name_x = esp_x + ew + esp_gap + slide;
        let cat_name_x = cat_x - cat_gap - cat_name_w - slide;
        let esp_name_y = cy - travel_max - esp_name_h / 2;
        let cat_name_y = cy + travel_max - cat_name_h / 2;

        // The heart, then the marks, then the names. What has come to a
        // stand-still is drawn one last time and then left alone.
        if !self.settled.heart {
            let heart_at = Point::new(heart_x, heart_y);
            self.blit(target, &self.heart, heart_at, heart_color, fade);
            if fade >= 1.0 {
                // The heart is as bright as it gets: it stays on the screen.
                self.settled.heart = true;
            } else {
                self.remember(Rectangle::new(heart_at, Size::new(hw as u32, hh as u32)));
            }
        }

        if !self.settled.marks {
            let esp_at = Point::new(esp_x, esp_y);
            self.blit(target, &self.espressif, esp_at, espressif_color, fade);
            let cat_at = Point::new(cat_x, cat_y);
            self.blit(target, &self.cactus, cat_at, cactus_color, fade);
            if split >= 1.0 {
                // The marks have arrived: they stay on the screen.
                self.settled.marks = true;
            } else {
                self.remember(Rectangle::new(esp_at, Size::new(ew as u32, eh as u32)));
                self.remember(Rectangle::new(cat_at, Size::new(cw as u32, ch as u32)));
            }
        }

        if !self.settled.names && name > 0.0 {
            let level = (name * 255.0) as u8;
            let style = MonoTextStyle::new(font, rgb(level, level, level));
            draw_name(
                target,
                ESPRESSIF_NAME,
                Point::new(esp_name_x, esp_name_y),
                style,
                scale,
            );

            let style = MonoTextStyle::new(font_b, rgb(level, level, level));
            draw_name(
                target,
                CACTUS_NAME,
                Point::new(cat_name_x, cat_name_y),
                style,
                scale,
            );
            if name >= 1.0 {
                // The names have arrived: they stay on the screen.
                self.settled.names = true;
            } else {
                self.remember(Rectangle::new(
                    Point::new(esp_name_x, esp_name_y),
                    Size::new(esp_name_w as u32, esp_name_h as u32),
                ));
                self.remember(Rectangle::new(
                    Point::new(cat_name_x, cat_name_y),
                    Size::new(cat_name_w as u32, cat_name_h as u32),
                ));
            }
        }
    }

    /// Draws a mark, tinted and faded.
    fn blit<D: DrawTarget<Color = Rgb565>>(
        &self,
        target: &mut D,
        mask: &Mask,
        at: Point,
        tint: (u8, u8, u8),
        fade: f32,
    ) {
        let fade = (fade * 255.0) as u16;
        let _ = target.draw_iter(MaskPixels {
            mask,
            at,
            index: 0,
            tint: rgb(tint.0, tint.1, tint.2),
            fade,
        });
    }

    /// Remembers an area, so that the next frame can erase it.
    fn remember(&mut self, rect: Rectangle) {
        if self.painted_len < self.painted.len() {
            self.painted[self.painted_len] = rect;
            self.painted_len += 1;
        }
    }
}

/// Draws a name, `scale` times as big as the font is. The fonts that come
/// with embedded-graphics are small (10x20 at most), which is too small for
/// a name next to a big mark on the Korvo's 7" screen.
fn draw_name<D: DrawTarget<Color = Rgb565>>(
    target: &mut D,
    name: &str,
    at: Point,
    style: MonoTextStyle<'_, Rgb565>,
    scale: u32,
) {
    match scale {
        2 => {
            let mut scaled = Scaled::<_, 2>::new(target, at);
            let _ = Text::with_baseline(name, Point::zero(), style, Baseline::Top).draw(&mut scaled);
        }
        3 => {
            let mut scaled = Scaled::<_, 3>::new(target, at);
            let _ = Text::with_baseline(name, Point::zero(), style, Baseline::Top).draw(&mut scaled);
        }
        _ => {
            let _ = Text::with_baseline(name, at, style, Baseline::Top).draw(target);
        }
    }
}

/// A draw target that draws everything `S` times as big, at a place.
struct Scaled<'a, D, const S: u32> {
    target: &'a mut D,
    origin: Point,
}

impl<'a, D, const S: u32> Scaled<'a, D, S> {
    fn new(target: &'a mut D, origin: Point) -> Self {
        Scaled { target, origin }
    }
}

impl<D: DrawTarget<Color = Rgb565>, const S: u32> OriginDimensions for Scaled<'_, D, S> {
    fn size(&self) -> Size {
        // The text is drawn at the origin, so this only has to be big enough
        // not to clip it.
        Size::new(4096, 4096)
    }
}

impl<D: DrawTarget<Color = Rgb565>, const S: u32> DrawTarget for Scaled<'_, D, S> {
    type Color = Rgb565;
    type Error = D::Error;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        let origin = self.origin;
        let scaled = pixels.into_iter().flat_map(move |Pixel(p, color)| {
            // Room for the biggest scale that is used.
            let mut block = [None; 16];
            for dy in 0..S {
                for dx in 0..S {
                    block[(dy * S + dx) as usize] = Some(Pixel(
                        Point::new(
                            origin.x + p.x * S as i32 + dx as i32,
                            origin.y + p.y * S as i32 + dy as i32,
                        ),
                        color,
                    ));
                }
            }
            block.into_iter().flatten()
        });
        self.target.draw_iter(scaled)
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let scaled = Rectangle::new(
            Point::new(
                self.origin.x + area.top_left.x * S as i32,
                self.origin.y + area.top_left.y * S as i32,
            ),
            Size::new(area.size.width * S, area.size.height * S),
        );
        self.target.fill_solid(&scaled, color)
    }
}

/// The lit pixels of a mark, tinted and faded.
struct MaskPixels<'a> {
    mask: &'a Mask,
    at: Point,
    index: usize,
    tint: Rgb565,
    fade: u16,
}

impl Iterator for MaskPixels<'_> {
    type Item = Pixel<Rgb565>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.index < self.mask.data.len() {
            let index = self.index;
            self.index += 1;
            let coverage = u16::from(self.mask.data[index]);
            let level = coverage * self.fade / 255;
            if level == 0 {
                continue;
            }
            let color = fade_color(self.tint, level);
            let px = (index % usize::from(self.mask.width)) as i32;
            let py = (index / usize::from(self.mask.width)) as i32;
            return Some(Pixel(Point::new(self.at.x + px, self.at.y + py), color));
        }
        None
    }
}

/// A colour with 8-bit channels, as an RGB565 colour.
///
/// `Rgb565::new` takes the channels in the width of the colour's own bits
/// (5, 6 and 5), so 8-bit channels have to be scaled down first.
fn rgb(r: u8, g: u8, b: u8) -> Rgb565 {
    Rgb565::new(
        (u16::from(r) * 31 / 255) as u8,
        (u16::from(g) * 63 / 255) as u8,
        (u16::from(b) * 31 / 255) as u8,
    )
}

/// Scales a colour down towards black, keeping its hue.
///
/// The channels are scaled in the width of the colour's own bits, so that a
/// faded red stays red instead of drifting to a different colour.
fn fade_color(color: Rgb565, level: u16) -> Rgb565 {
    let level = level.min(255);
    Rgb565::new(
        (u16::from(color.r()) * level / 255) as u8,
        (u16::from(color.g()) * level / 255) as u8,
        (u16::from(color.b()) * level / 255) as u8,
    )
}

/// 0 before `from`, 1 after `to`, smooth in between.
fn ramp(ms: f32, from: f32, to: f32) -> f32 {
    let x = ((ms - from) / (to - from)).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}
