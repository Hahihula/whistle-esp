//! Renders the boot splash on the host, so the composition can be looked at
//! without flashing a board. It draws with the very code the firmwares run
//! (common/src/splash.rs), so what comes out here is what the screen shows.
//!
//!     cargo run --release -- [vocat|korvo]
//!
//! Writes a PPM per board (one frame per cell of a grid, the frames in the
//! order of FRAMES).

// The splash itself, as the firmware has it.
#[path = "../../../common/src/splash.rs"]
#[allow(dead_code)]
mod splash;

use std::fs::File;
use std::io::{BufWriter, Write};

use embedded_graphics::{
    geometry::{OriginDimensions, Size},
    pixelcolor::{Rgb565, raw::RawU16},
    prelude::*,
    primitives::Rectangle,
};

use splash::Splash;

/// The moments the frames are taken at, in milliseconds.
const FRAMES: [u32; 10] = [0, 150, 400, 700, 1000, 1300, 1600, 1900, 2200, 2600];

/// A frame buffer to draw into.
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

    /// The image as a binary PPM (P6).
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

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
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

/// Renders the frames of one board into a grid.
fn render(name: &str, mut splash: Splash, width: u32, height: u32) {
    let cols = 5;
    let rows = (FRAMES.len() + cols - 1) / cols as usize;
    let mut grid = Canvas::new(width * cols as u32, height * rows as u32);
    // One canvas for the whole animation, like the screen of the board: what
    // the splash leaves on the screen stays there.
    let mut canvas = Canvas::new(width, height);

    for (index, ms) in FRAMES.iter().enumerate() {
        splash.draw(&mut canvas, core::time::Duration::from_millis(*ms as u64));
        let x = (index % cols) as u32 * width;
        let y = (index / cols) as u32 * height;
        for row in 0..height {
            for col in 0..width {
                let pixel = canvas.buf[(row * width + col) as usize];
                grid.put(x + col, y + row, Rgb565::from(RawU16::new(pixel)));
            }
        }
        println!("{name}: frame {index} at {ms} ms");
    }

    let path = format!("/tmp/splash_{name}.ppm");
    grid.write_ppm(&path);
    println!("{name}: wrote {path}");
}

fn main() {
    let which = std::env::args().nth(1).unwrap_or_else(|| "both".to_string());
    if which == "vocat" || which == "both" {
        render("vocat", Splash::vocat(), 360, 360);
    }
    if which == "korvo" || which == "both" {
        render("korvo", Splash::korvo(), 800, 480);
    }
}
