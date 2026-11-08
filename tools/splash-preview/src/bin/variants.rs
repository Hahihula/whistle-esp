//! Renders the last frame of the splash for several typographies, so the
//! composition can be compared before it is picked.
//!
//!     cargo run --release --bin variants

// The splash itself, as the firmware has it.
#[path = "../../../../common/src/splash.rs"]
#[allow(dead_code)]
mod splash;

use std::fs::File;
use std::io::{BufWriter, Write};

use embedded_graphics::{
    geometry::{OriginDimensions, Size},
    mono_font::{MonoFont, ascii},
    pixelcolor::{Rgb565, raw::RawU16},
    prelude::*,
    primitives::Rectangle,
};

use splash::{Config, Mask, Splash};

struct Canvas {
    width: u32,
    height: u32,
    buf: Vec<u16>,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Self {
        Canvas {
            width,
            height,
            buf: vec![0; (width * height) as usize],
        }
    }

    fn put(&mut self, x: u32, y: u32, color: Rgb565) {
        if x < self.width && y < self.height {
            self.buf[(y * self.width + x) as usize] = RawU16::from(color).into_inner();
        }
    }

    fn write_ppm(&self, path: &str) {
        let mut out = BufWriter::new(File::create(path).expect("can write the PPM"));
        write!(out, "P6\n{} {}\n255\n", self.width, self.height).expect("PPM header");
        for pixel in &self.buf {
            let raw = *pixel;
            let r = (raw >> 8) & 0xF8;
            let g = (raw >> 3) & 0xFC;
            let b = (raw << 3) & 0xF8;
            out.write_all(&[r as u8, g as u8, b as u8]).expect("PPM pixels");
        }
    }
}

impl OriginDimensions for Canvas {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl DrawTarget for Canvas {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if point.x < 0 || point.y < 0 {
                continue;
            }
            self.put(point.x as u32, point.y as u32, color);
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), Self::Error> {
        let area = area.intersection(&self.bounding_box());
        let Some(bottom_right) = area.bottom_right() else {
            return Ok(());
        };
        for y in area.top_left.y.max(0) as u32..=bottom_right.y.min(self.height as i32 - 1) as u32 {
            for x in area.top_left.x.max(0) as u32..=bottom_right.x.min(self.width as i32 - 1) as u32 {
                self.put(x, y, color);
            }
        }
        Ok(())
    }
}

fn vocat(font: &'static MonoFont<'static>, font_b: &'static MonoFont<'static>, scale: u32) -> Splash {
    Splash::new(
        Mask::new(include_bytes!("../../../../common/assets/vocat/splash_espressif.msk")),
        Mask::new(include_bytes!("../../../../common/assets/vocat/splash_cactus.msk")),
        Mask::new(include_bytes!("../../../../common/assets/vocat/splash_heart.msk")),
        Config {
            width: 360,
            height: 360,
            font,
            font_b,
            name_scale: scale,
            travel: 62,
            name_gap: 8,
            espressif_color: (235, 24, 32),
            cactus_color: (255, 255, 255),
            heart_color: (255, 64, 96),
        },
    )
}

fn korvo(font: &'static MonoFont<'static>, font_b: &'static MonoFont<'static>, scale: u32) -> Splash {
    Splash::new(
        Mask::new(include_bytes!("../../../../common/assets/korvo/splash_espressif.msk")),
        Mask::new(include_bytes!("../../../../common/assets/korvo/splash_cactus.msk")),
        Mask::new(include_bytes!("../../../../common/assets/korvo/splash_heart.msk")),
        Config {
            width: 800,
            height: 480,
            font,
            font_b,
            name_scale: scale,
            travel: 76,
            name_gap: 16,
            espressif_color: (235, 24, 32),
            cactus_color: (255, 255, 255),
            heart_color: (255, 64, 96),
        },
    )
}

fn main() {
    let variants: [(&str, &MonoFont, &MonoFont, u32); 4] = [
        ("8x13 + 6x13", &ascii::FONT_8X13, &ascii::FONT_6X13, 2),
        ("10x20 + 6x13", &ascii::FONT_10X20, &ascii::FONT_6X13, 2),
        ("10x20 + 8x13", &ascii::FONT_10X20, &ascii::FONT_8X13, 2),
        ("8x13 + 8x13", &ascii::FONT_8X13, &ascii::FONT_8X13, 2),
    ];

    for (name, font, font_b, scale) in variants {
        let mut canvas = Canvas::new(800, 480);
        let mut splash = korvo(font, font_b, scale);
        splash.draw(&mut canvas, core::time::Duration::from_millis(2600));
        let path = format!("/tmp/variant_korvo_{}.ppm", name.replace(' ', "_"));
        canvas.write_ppm(&path);
        println!("korvo {name}: {path}");

        let mut canvas = Canvas::new(360, 360);
        let mut splash = vocat(font, font_b, scale);
        splash.draw(&mut canvas, core::time::Duration::from_millis(2600));
        let path = format!("/tmp/variant_vocat_{}.ppm", name.replace(' ', "_"));
        canvas.write_ppm(&path);
        println!("vocat {name}: {path}");
    }
}
