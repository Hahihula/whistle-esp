//! Whistle speech recognition on the ESP-VoCat v1.2.
//!
//! Hold a touch pad (either side of the cat's head), speak, let go: the
//! transcript appears on the round LCD and on the console. Everything runs on
//! the board. (The BOOT button is inside the case on the devkit.)
//!
//! - core 0: Embassy. Microphones (ES7210 over I2S, drained continuously,
//!   since the DMA ring holds only ~128 ms), the touch pads and the LCD.
//! - core 1: Whistle, blocking, with the PIE vector unit enabled. The model is
//!   read in place from its flash partition, mapped after PSRAM.

#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

extern crate alloc;

mod accel;
mod bench;
mod qio;
mod ui;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
use esp_hal::{system::Stack, timer::timg::TimerGroup};
use log::{error, info, warn};
use static_cell::ConstStaticCell;
use cactus_whistle::{Spec, par::Int8Help};

// Board support: the esp-vocat library from esp-vocat-rust-examples.
use esp_vocat::{
    audio::{AudioIn, I2sInPins},
    board::{self, LcdReset},
    display::{Backlight, FrameBuffer, Lcd},
    es7210::{Es7210, Gain},
    touch::{TouchEvent, TouchInput, TouchPads},
};

use whistle_common::{
    SAMPLE_RATE,
    audio::{self, MAX_CLIP},
    inference::{self, Config, FROM_INFERENCE, Msg, TO_INFERENCE},
};

use crate::ui::Screen;

esp_bootloader_esp_idf::esp_app_desc!();

/// Internal heap on top of the RAM the bootloader used (board::RECLAIMED_HEAP).
const INTERNAL_HEAP: usize = 128 * 1024;
const MIC_GAIN_DB: f32 = 30.0;
/// Run the matmul micro-benchmark (bench.rs) at boot.
const BENCH: bool = false;
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
const HINT: &str = "Hold a touch pad and speak";
/// Speech gate: windows this far over the noise floor count as speech.
const GATE_DB: f32 = 12.0;

/// Spoken by macOS `say`, the same clip the host tests use. Always in the
/// image: the QIO switch (qio.rs) also reads it back as its known-content
/// check, and with the `selftest` feature it is transcribed at boot.
static SELFTEST_WAV: &[u8] = include_bytes!("../assets/selftest_en.wav");

/// Set while a touch pad is held: mic_task keeps the samples.
static RECORDING: AtomicBool = AtomicBool::new(false);
static CLIP: Signal<CriticalSectionRawMutex, Vec<i16>> = Signal::new();
static CORE1_STACK: ConstStaticCell<Stack<{ 96 * 1024 }>> = ConstStaticCell::new(Stack::new());

#[allow(clippy::large_stack_frames, reason = "main holds the LCD and codec futures")]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let p = board::init();
    board::init_logger();
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: board::RECLAIMED_HEAP);
    // More internal SRAM for the engine, ahead of PSRAM in the allocator's
    // order: small hot buffers (int8 activations, decoded rows, MLP weights)
    // are read for every weight row and should not go through the PSRAM cache.
    esp_alloc::heap_allocator!(size: INTERNAL_HEAP);
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    board::wait_for_console().await;
    info!("=== Whistle speech recognition on ESP-VoCat ({}) ===", SPEC.name());
    // Before PSRAM and the second core: see qio.rs.
    match qio::enable(SELFTEST_WAV) {
        Ok(r) => info!("flash: QIO on, {r:x?}"),
        Err(e) => warn!("flash: staying in DIO: {e:x?}"),
    }

    // PSRAM into the heap, then the model partition mapped right after it.
    let psram = board::psram(p.PSRAM);
    let (pstart, psize) = psram.raw_parts();
    esp_alloc::psram_allocator!(&psram);
    info!("PSRAM: {} KiB at {:#x}", psize / 1024, pstart as usize);
    let model = match unsafe {
        accel::map_flash(pstart as usize + psize, accel::MODEL_FLASH_OFFSET, accel::MODEL_PARTITION_SIZE)
    } {
        Ok(m) => m,
        Err(e) => halt_log("cannot map the model partition", e).await,
    };
    let tag = u32::from_le_bytes([model[0], model[1], model[2], model[3]]);
    info!("model partition mapped at {:#x}, tag {tag:#010x}", model.as_ptr() as usize);
    let model_ok = tag == cactus_whistle::cact::TAG4;

    let _power = board::peripheral_power_on(p.GPIO9);
    let _codec_power = board::codec_power_on(p.GPIO48);
    Timer::after(Duration::from_millis(20)).await;

    // I2S first: the ES7210 needs MCLK before it can be configured.
    let pins = I2sInPins { mclk: p.GPIO42, bclk: p.GPIO40, ws: p.GPIO39, din: p.GPIO3 };
    let input = match AudioIn::new(p.I2S0, p.DMA_CH1, pins, SAMPLE_RATE) {
        Ok(i) => i,
        Err(e) => halt_log("I2S init failed", e).await,
    };
    spawner.spawn(mic_task(input).unwrap());
    spawner.spawn(helper_task().unwrap());

    let i2c = board::i2c(p.I2C0, p.GPIO1, p.GPIO2).expect("I2C config is valid");
    let mut adc = Es7210::new(i2c);
    if let Err(e) = adc.init(Gain::from_db(MIC_GAIN_DB)).await {
        warn!("ES7210 init failed: {e:?}");
    }

    let mut backlight = Backlight::new(p.LEDC, p.GPIO44);
    let mut reset = LcdReset::new(p.GPIO47);
        let mut lcd = match Lcd::new(p.SPI2, p.DMA_CH0, esp_vocat::lcd_pins!(p), &mut reset).await {
        Ok(l) => l,
        Err(e) => halt_log("LCD init failed", e).await,
    };
    let mut fb = FrameBuffer::new();
    let mut screen = Screen::new();

    if !model_ok {
        error!("no Whistle model in flash. Flash it with:");
        error!("  cargo flash-model vocat   (in the repository root)");
        screen.status("No model in flash", ui::RED);
        screen.body("cargo flash-model vocat");
        screen.draw(&mut fb);
        let _ = lcd.flush(&mut fb).await;
        backlight.fade(0, 100, Duration::from_millis(400));
        halt().await;
    }

    screen.status("Loading model...", ui::GREY);
    screen.draw(&mut fb);
    let _ = lcd.flush(&mut fb).await;
    backlight.fade(0, 100, Duration::from_millis(400));

    esp_rtos::start_second_core(p.CPU_CTRL, p.FROM_CPU_INTR1, CORE1_STACK.take(), move || {
        inference::inference_core(
            model,
            Config {
                spec: SPEC,
                selftest_wav: SELFTEST.then_some(SELFTEST_WAV),
                compare_specs: COMPARE_SPECS,
                gate_db: GATE_DB,
                heap_free: || esp_alloc::HEAP.free(),
                after_open: if BENCH { Some(bench::run) } else { None },
            },
        )
    });

    // Measures the pad baselines: nobody should touch them for ~200 ms.
    let pads = TouchPads::new(p.SENS, p.RTC_IO, p.GPIO6, p.GPIO7);
    let mut touch = TouchInput::new(pads).await;
    let mut ready = false;
    let mut busy = false;

    loop {
        // Messages from the inference core (polled: it never awaits).
        while let Ok(msg) = FROM_INFERENCE.try_receive() {
            match msg {
                Msg::Status(s) => screen.status(s, ui::GREY),
                Msg::Ready { load_ms } => {
                    ready = true;
                    info!("model ready in {load_ms} ms");
                    screen.status("Ready", ui::GREY);
                }
                Msg::Result(t) => {
                    busy = false;
                    info!(
                        "{}[{}] {} | audio {} ms, encode {} ms, decode {} ms ({} tokens), enc_fnv {:08x}",
                        if t.selftest { "self-test " } else { "" },
                        t.lang,
                        t.text,
                        t.audio_ms,
                        t.enc_ms,
                        t.dec_ms,
                        t.tokens,
                        t.fnv
                    );
                    screen.transcript(&t);
                }
                Msg::NoSpeech => {
                    busy = false;
                    screen.status("No speech heard", ui::AMBER);
                }
                Msg::Error(e) => {
                    busy = false;
                    error!("{e}");
                    screen.status("Error", ui::RED);
                    screen.body(&e);
                }
            }
            if ready && !busy {
                screen.hint(HINT);
            }
            screen.draw(&mut fb);
            let _ = lcd.flush(&mut fb).await;
        }

        let Either::Second(TouchEvent::Down(pad)) =
            select(Timer::after(Duration::from_millis(50)), touch.next_event()).await
        else {
            continue;
        };
        if !ready || busy {
            continue;
        }
        // Recording while the pad is held.
        RECORDING.store(true, Ordering::Release);
        let t0 = Instant::now();
        screen.status("Listening...", ui::GREEN);
        screen.hint("let go when done");
        screen.draw(&mut fb);
        let _ = lcd.flush(&mut fb).await;
        let released = async {
            loop {
                if let TouchEvent::Up(p, _) = touch.next_event().await
                    && p == pad
                {
                    return;
                }
            }
        };
        let _ = select(released, Timer::after(Duration::from_secs(10))).await;
        RECORDING.store(false, Ordering::Release);
        let clip = CLIP.wait().await;
        info!("recorded {} ms ({} samples)", t0.elapsed().as_millis(), clip.len());
        if let Some(x) = audio::prepare(&clip) {
            TO_INFERENCE.signal(x);
            busy = true;
            screen.status("Thinking...", ui::BLUE);
            screen.hint("");
        } else {
            screen.status("Too short", ui::AMBER);
            screen.hint(HINT);
        }
        screen.draw(&mut fb);
        let _ = lcd.flush(&mut fb).await;
    }
}

/// Core 0's share of the matmuls (whistle_common::inference::helper).
#[embassy_executor::task]
async fn helper_task() {
    inference::helper(Int8Help::Full).await // both S3 cores have PIE
}

/// Reads the microphones forever (the DMA stops if the ring overflows) and
/// keeps MIC1 while [`RECORDING`] is set; the clip goes out through [`CLIP`].
#[embassy_executor::task]
async fn mic_task(mut input: AudioIn<'static>) {
    let mut clip: Vec<i16> = Vec::new();
    let mut was_recording = false;
    loop {
        let rec = RECORDING.load(Ordering::Acquire);
        if rec && !was_recording {
            clip = Vec::with_capacity(MAX_CLIP);
        }
        let result = input
            .read(|l, _r| {
                if rec && clip.len() < MAX_CLIP {
                    clip.push(l);
                }
            })
            .await;
        if was_recording && !rec {
            CLIP.signal(core::mem::take(&mut clip));
        }
        was_recording = rec;
        if let Err(e) = result {
            warn!("mic stream error: {e:?}");
            Timer::after(Duration::from_millis(100)).await;
        }
    }
}

async fn halt_log(what: &str, e: impl core::fmt::Debug) -> ! {
    error!("{what}: {e:?}");
    halt().await
}

async fn halt() -> ! {
    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}
