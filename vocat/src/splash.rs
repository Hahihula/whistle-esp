//! The boot splash on the round 360x360 screen of the ESP-VoCat.

use embassy_time::{Duration, Instant, Timer};
use esp_vocat::display::{FrameBuffer, Lcd};

use whistle_common::splash::{DURATION, Splash};

/// Shows the splash on the LCD until it is over.
pub async fn run(lcd: &mut Lcd<'_>, fb: &mut FrameBuffer) {
    let mut splash = Splash::vocat();
    let t0 = Instant::now();
    // About 30 frames a second; the flush of the changed rows is quick.
    let frame = Duration::from_millis(33);
    loop {
        let t = t0.elapsed();
        splash.draw(fb, t.into());
        let _ = lcd.flush(fb).await;
        let elapsed: core::time::Duration = t.into();
        if elapsed >= DURATION {
            return;
        }
        let spent = t0.elapsed() - t;
        if spent < frame {
            Timer::after(frame - spent).await;
        }
    }
}
