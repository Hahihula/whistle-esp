//! Whistle on the ESP32-S31-Korvo-1.
//!
//! - core 1: Whistle, with PIE (the S31's PIE exists only on core 1; ESP-IDF
//!   portasm.S: "On the ESP32-S31, the PIE is only available on core 1").
//! - core 0: Embassy. The 800x480 LCD and a helper task that takes float rows
//!   and the decoder's unpack-bound int8 rows (plain dot product).
//!
//! With the `selftest` feature it first transcribes a built-in clip. Then hold any of
//! the four buttons (VOL+, VOL-, PLAY, SET), speak, and let go: the two
//! microphones go through the ES8389 codec and I2S, Whistle transcribes MIC1
//! and the LCD shows the text. The output head (read in full for every token)
//! is copied to PSRAM:
//! measured with the LCD streaming its frame buffer out of PSRAM, decoding
//! took 7.5 s with the head in PSRAM and 10.5 s with it in (DIO) flash.

#![no_std]
#![no_main]

extern crate alloc;

mod mmap;
#[cfg(feature = "splash")]
mod splash;
mod ui;

use alloc::{format, vec::Vec};
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Timer};
use embedded_graphics::{pixelcolor::Rgb565, prelude::*};
use esp_backtrace as _;
use esp_hal::{clock::CpuClock, psram::Psram, system::Stack, timer::timg::TimerGroup};
use log::{error, info, warn};
use static_cell::ConstStaticCell;
use cactus_whistle::{Spec, par::Int8Help};

use esp32_s31_korvo::{
    audio::{AudioIn, I2sInPins},
    buttons::{Buttons, Event},
    display::{Display, FRAME_BYTES},
    es8389::{Es8389, Gain},
    lcd_pins,
};
use whistle_common::{
    SAMPLE_RATE,
    audio::{self, MAX_CLIP, peak_db, rms_db},
    inference::{self, Config, FROM_INFERENCE, Msg, TO_INFERENCE},
};

/// First try at 24.5 dB gave speech peaks of only -24 dBFS (-47 dBFS rms).
const MIC_GAIN_DB: f32 = 33.5;
/// LCD pixel clock: 15 MHz, ~29.5 frames/s (see Display::with_pixel_clock).
/// The panel's minimum is above 12 MHz (it shows its own colour-bar test
/// pattern there).
const LCD_PIXEL_CLOCK: esp_hal::time::Rate = esp_hal::time::Rate::from_mhz(15);
/// Speech gate: windows this far over the noise floor count as speech.
const GATE_DB: f32 = 12.0;
/// With the `selftest` feature: transcribe the built-in clip at boot (about
/// 16-20 s; for development and automated checks). Without it the board is
/// ready for recordings as soon as the model is open.
const SELFTEST: bool = cfg!(feature = "selftest");
/// With `selftest`: run the self-test under each spec and log the times
/// (README).
const COMPARE_SPECS: bool = false;
/// Spec for transcribing: the porting guide's, 5-beam search, or greedy with
/// `--features fast` (README: measured against Cactus's engine).
pub const SPEC: Spec = Spec::DEFAULT;
const HINT: &str = "Hold any button (VOL+, VOL-, PLAY, SET), speak, let go";

static RECORDING: AtomicBool = AtomicBool::new(false);
static CLIP: Signal<CriticalSectionRawMutex, (Vec<i16>, Vec<i16>)> = Signal::new();

esp_bootloader_esp_idf::esp_app_desc!();

/// Spoken by macOS `say`, the same clip the host tests and the VoCat use.
/// Only in the image with the `selftest` feature.
#[cfg(feature = "selftest")]
static SELFTEST_WAV: &[u8] = include_bytes!("../assets/selftest_en.wav");
#[cfg(not(feature = "selftest"))]
static SELFTEST_WAV: &[u8] = &[];
static CORE1_STACK: ConstStaticCell<Stack<{ 96 * 1024 }>> = ConstStaticCell::new(Stack::new());

#[allow(clippy::large_stack_frames, reason = "main holds the display")]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    // Internal SRAM for the engine's small hot buffers, ahead of PSRAM.
    esp_alloc::heap_allocator!(size: 160 * 1024);
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0);
    Timer::after(Duration::from_millis(500)).await;
    info!("=== Cactus Whistle on ESP32-S31-Korvo-1 ({}) ===", SPEC.name());

    // PSRAM: the LCD frame buffer first, the rest to the heap.
    let psram = Psram::new(p.PSRAM, Default::default());
    let (pstart, psize) = psram.raw_parts();
    let fb_len = FRAME_BYTES.next_multiple_of(64);
    // SAFETY: the PSRAM mapping is valid for `psize` bytes for the rest of
    // the program; the frame buffer and the heap region don't overlap.
    let framebuffer = unsafe {
        esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
            pstart.add(fb_len),
            psize - fb_len,
            esp_alloc::MemoryCapability::External.into(),
        ));
        core::slice::from_raw_parts_mut(pstart, fb_len)
    };
    info!("PSRAM: {} KiB at {:#x}, {} KiB frame buffer", psize / 1024, pstart as usize, fb_len / 1024);

    // A lower refresh than the driver's default (~29.5 instead of ~35
    // frames/s): the scan-out then takes ~30 MB/s of PSRAM instead of ~36,
    // which leaves more to the inference (about 2% faster). Under inference
    // load the DMA still falls behind now and then, so the picture drifts
    // slowly while it "thinks"; the next present() (the transcript) realigns
    // it. Correcting it during inference was tried: it blinks 2-3 times a
    // second, which looks worse.
    let mut lcd = match Display::with_pixel_clock(p.LCD_CAM, p.DMA_AXI_CH0, lcd_pins!(p), framebuffer, LCD_PIXEL_CLOCK) {
        Ok(d) => d,
        Err(e) => halt(&format!("display init failed: {e:?}")).await,
    };
    {
        // A black screen, so the scan-out starts on something. The splash
        // takes over from here.
        let mut c = lcd.canvas();
        let _ = c.clear(Rgb565::BLACK);
    }
    if let Err(e) = lcd.present().await {
        warn!("present: {e:?}");
    }

    let (model, entry) = match unsafe { mmap::map_flash(mmap::MODEL_FLASH_OFFSET, mmap::MODEL_PARTITION_SIZE) } {
        Ok(m) => m,
        Err(e) => halt(&format!("cannot map the model: {e:?}")).await,
    };
    let tag = u32::from_le_bytes([model[0], model[1], model[2], model[3]]);
    info!(
        "model mapped at {:#x} (flash MMU entry {entry}), tag {tag:#010x}",
        model.as_ptr() as usize
    );
    if tag != cactus_whistle::cact::TAG4 {
        error!("no Whistle model in flash; flash it with:");
        error!("  cargo flash-model korvo   (in the repository root)");
        {
            let mut c = lcd.canvas();
            ui::draw_frame(&mut c);
            ui::status(&mut c, "No model in flash", ui::RED);
            ui::body(&mut c, "cargo flash-model korvo", false);
        }
        let _ = lcd.present().await;
        halt("no model").await;
    }

    // I2S first: the ES8389 is a slave clocked from BCLK.
    let pins = I2sInPins { bclk: p.GPIO3, ws: p.GPIO4, din: p.GPIO6 };
    match AudioIn::new(p.I2S0, p.DMA_CH1, pins, SAMPLE_RATE) {
        Ok(input) => spawner.spawn(mic_task(input).unwrap()),
        Err(e) => error!("I2S init failed: {e:?}"),
    }
    Timer::after(Duration::from_millis(20)).await;
    let i2c = esp_hal::i2c::master::I2c::new(
        p.I2C0,
        esp_hal::i2c::master::Config::default().with_frequency(esp_hal::time::Rate::from_khz(100)),
    )
    .expect("I2C config is valid")
    .with_sda(p.GPIO0)
    .with_scl(p.GPIO1)
    .into_async();
    let mut codec = Es8389::new(i2c);
    match codec.chip_id().await {
        Ok(id) => info!("ES8389 chip ID {:02X} {:02X}", id[0], id[1]),
        Err(e) => error!("ES8389 did not answer: {e:?}"),
    }
    match codec.init(Gain::from_db(MIC_GAIN_DB)).await {
        Ok(()) => info!("ES8389 running, mic gain {} dB", Gain::from_db(MIC_GAIN_DB).db()),
        Err(e) => error!("ES8389 init failed: {e:?}"),
    }
    let mut buttons = match Buttons::new(p.ADC1, p.GPIO42) {
        Ok(b) => b,
        Err(e) => halt(&format!("buttons: {e:?}")).await,
    };

    // The splash: espressif, a heart, cactuscompute.
    //
    // It plays before the model is loaded on the second core. The load
    // copies a few megabytes of the model's output head through PSRAM, and
    // PSRAM has little headroom: the LCD's scan-out falls behind, the
    // picture drifts and the driver has to realign it with a blink. On the
    // quiet PSRAM of the splash the picture stays where it belongs.
    #[cfg(feature = "splash")]
    splash::run(&mut lcd).await;

    esp_rtos::start_second_core(p.CPU_CTRL, CORE1_STACK.take(), move || {
        inference::inference_core(
            model,
            Config {
                spec: SPEC,
                selftest_wav: SELFTEST.then_some(SELFTEST_WAV),
                compare_specs: COMPARE_SPECS,
                gate_db: GATE_DB,
                heap_free: || esp_alloc::HEAP.free(),
                after_open: None,
            },
        )
    });
    spawner.spawn(helper_task().unwrap());

    // The splash covered the screen, so the frame is drawn again.
    {
        let mut c = lcd.canvas();
        ui::draw_frame(&mut c);
        ui::status(&mut c, "Loading model...", ui::GREY);
    }
    if let Err(e) = lcd.present().await {
        warn!("present: {e:?}");
    }

    let mut ready = false;
    let mut busy = SELFTEST; // with the self-test, it runs first

    loop {
        let msg = match select(FROM_INFERENCE.receive(), buttons.next_event()).await {
            Either::First(m) => m,
            Either::Second(Event::Pressed { button, .. }) if ready && !busy => {
                // Record while the button is held.
                RECORDING.store(true, Ordering::Release);
                let t0 = embassy_time::Instant::now();
                {
                    let mut c = lcd.canvas();
                    ui::status(&mut c, "Listening...  let go when done", ui::GREEN);
                }
                let _ = lcd.present().await;
                let released = async {
                    loop {
                        if let Event::Released { button: b, .. } = buttons.next_event().await
                            && b == button
                        {
                            return;
                        }
                    }
                };
                let _ = select(released, Timer::after(Duration::from_secs(10))).await;
                RECORDING.store(false, Ordering::Release);
                let (mic1, mic2) = CLIP.wait().await;
                info!(
                    "recorded {} ms with {}: MIC1 {:.1} dBFS rms / {:.1} peak, MIC2 {:.1} / {:.1}",
                    t0.elapsed().as_millis(),
                    button.name(),
                    rms_db(&mic1),
                    peak_db(&mic1),
                    rms_db(&mic2),
                    peak_db(&mic2)
                );
                let mut c = lcd.canvas();
                if let Some(x) = audio::prepare(&mic1) {
                    TO_INFERENCE.signal(x);
                    busy = true;
                    ui::status(&mut c, "Thinking...", ui::BLUE);
                } else {
                    ui::status(&mut c, "Too short", ui::AMBER);
                }
                drop(c);
                let _ = lcd.present().await;
                continue;
            }
            Either::Second(_) => continue,
        };
        {
            let mut c = lcd.canvas();
            match msg {
                Msg::Status(s) => ui::status(&mut c, s, ui::BLUE),
                Msg::Ready { load_ms } => {
                    info!("model ready in {load_ms} ms");
                    ui::status(&mut c, "Model ready", ui::GREY);
                    ready = true;
                }
                Msg::NoSpeech => {
                    busy = false;
                    ui::status(&mut c, "No speech heard", ui::AMBER);
                }
                Msg::Result(r) => {
                    busy = false;
                    info!(
                        "{} [{}] {} | audio {} ms, encode {} ms, decode {} ms ({} tokens), enc_fnv {:08x}, LCD resyncs so far {}",
                        if r.selftest { "self-test" } else { "heard" },
                        r.lang,
                        r.text,
                        r.audio_ms,
                        r.enc_ms,
                        r.dec_ms,
                        r.tokens,
                        r.fnv,
                        lcd_resyncs()
                    );
                    if r.selftest {
                        ui::status(&mut c, "Self-test (built-in English clip)", ui::GREEN);
                    } else {
                        ui::status(&mut c, "Heard", ui::GREEN);
                    }
                    ui::body(&mut c, &r.text, r.lang == "pl");
                    ui::footer(
                        &mut c,
                        &format!(
                            "language {}  |  {:.1} s of audio  |  encode {:.1} s  |  decode {:.1} s  |  {} tokens",
                            r.lang,
                            r.audio_ms as f32 / 1000.0,
                            r.enc_ms as f32 / 1000.0,
                            r.dec_ms as f32 / 1000.0,
                            r.tokens
                        ),
                        &format!(
                            "{}  |  PIE on core 1, core 0 helping  |  encoder checksum {:08x}",
                            SPEC.name(),
                            r.fnv
                        ),
                    );
                }
                Msg::Error(e) => {
                    busy = false;
                    error!("{e}");
                    ui::status(&mut c, "Error", ui::RED);
                    ui::body(&mut c, &e, false);
                }
            }
        }
        if ready && !busy {
            let mut c = lcd.canvas();
            ui::hint(&mut c, HINT);
        }
        if let Err(e) = lcd.present().await {
            warn!("present: {e:?}");
        }
        LCD_RESYNCS.store(lcd.resyncs(), Ordering::Relaxed);
    }
}

static LCD_RESYNCS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn lcd_resyncs() -> u32 {
    LCD_RESYNCS.load(Ordering::Relaxed)
}

/// Core 0's share of the row work. The S31's PIE is on core 1 only, so core 0
/// takes float rows plus the decoder's unpack-bound int8 rows with the plain
/// dot product (whistle_common::inference::helper).
#[embassy_executor::task]
async fn helper_task() {
    inference::helper(Int8Help::Light).await
}

/// Reads the microphones forever (the DMA stops if the ring overflows) and
/// keeps both channels while [`RECORDING`] is set; the clip goes out through
/// [`CLIP`].
#[embassy_executor::task]
async fn mic_task(mut input: AudioIn<'static>) {
    let (mut mic1, mut mic2): (Vec<i16>, Vec<i16>) = (Vec::new(), Vec::new());
    let mut was_recording = false;
    loop {
        let rec = RECORDING.load(Ordering::Acquire);
        if rec && !was_recording {
            mic1 = Vec::with_capacity(MAX_CLIP);
            mic2 = Vec::with_capacity(MAX_CLIP);
        }
        let result = input
            .read(|l, r| {
                if rec && mic1.len() < MAX_CLIP {
                    mic1.push(l);
                    mic2.push(r);
                }
            })
            .await;
        if was_recording && !rec {
            CLIP.signal((core::mem::take(&mut mic1), core::mem::take(&mut mic2)));
        }
        was_recording = rec;
        if let Err(e) = result {
            warn!("mic stream error: {e:?}");
            Timer::after(Duration::from_millis(100)).await;
        }
    }
}

async fn halt(why: &str) -> ! {
    error!("{why}");
    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}
