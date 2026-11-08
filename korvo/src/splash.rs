//! The boot splash on the 800x480 screen of the ESP32-S31-Korvo-1.

use embassy_time::Instant;
use esp32_s31_korvo::display::Display;

use whistle_common::splash::{Splash, DURATION};

/// Shows the splash on the LCD until it is over.
pub async fn run(lcd: &mut Display<'_>) {
    let mut splash = Splash::korvo();
    // The screen is already black, so the first frame does not have to clear
    // the whole thing again: that is 768 kB of writing on a display that is
    // scanned out of PSRAM.
    splash.assume_cleared();
    let t0 = Instant::now();
    loop {
        let t = t0.elapsed();
        {
            let mut canvas = lcd.canvas();
            splash.draw(&mut canvas, t.into());
        }
        // The scan-out paces the frames: one present per LCD frame.
        let _ = lcd.present().await;
        let elapsed: core::time::Duration = t.into();
        if elapsed >= DURATION {
            return;
        }
    }
}
