//! Whistle on the ESP32-P4X-Function-EV-Board.
//!
//! - core 1: Whistle, with PIE.
//! - core 0: Embassy. The 7" LCD, the microphone, the BOOT button and a
//!   helper task that takes matmul rows off core 1. The P4 has PIE on both
//!   cores, so the helper takes any row with the PIE dot product, as on the
//!   ESP32-S3.
//!
//! With the `selftest` feature it first transcribes a built-in clip. Then
//! hold BOOT, speak, and let go: the on-board microphone goes through the
//! ES8311 codec and I2S (32 kHz, halved to Whistle's 16 kHz), Whistle
//! transcribes it and the LCD shows the text. The output head (read in full
//! for every token) is copied to PSRAM.
//!
//! The LCD is optional: without the adapter board connected the firmware
//! logs the transcripts only.

#![no_std]
#![no_main]

extern crate alloc;

mod decimate;
mod mmap;
mod ui;

use alloc::{format, vec::Vec};
use core::sync::atomic::{AtomicBool, Ordering};

use cactus_whistle::{Spec, par::Int8Help};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Delay, Duration, Timer};
use esp_backtrace as _;
use esp_hal::{
    gpio::Input,
    interrupt::Priority,
    peripherals::{DMA_CH0, I2S0},
    system::Stack,
    timer::timg::TimerGroup,
};
use esp_rtos::embassy::InterruptExecutor;
use log::{error, info, warn};
use static_cell::{ConstStaticCell, StaticCell};

use esp32_p4_function_ev::{
    audio::{
        self, AudioIn, I2sInPins,
        es8311::{MicGain, Mode, SampleRate},
    },
    board,
    display::{self, Display},
    i2s_in_pins, lcd_pins,
};
use whistle_common::{
    audio::{self as wav, MAX_CLIP, peak_db, rms_db},
    inference::{self, Config, FROM_INFERENCE, Msg, TO_INFERENCE},
};

use decimate::Decimator;
use ui::Screen;

/// The ES8311's pre-amplifier maximum, as Espressif's driver sets it.
const MIC_GAIN_DB: u8 = 30;
/// The microphone rate; [`Decimator`] halves it to Whistle's 16 kHz.
const MIC_RATE: SampleRate = SampleRate::Hz32000;
/// Speech gate: windows this far over the noise floor count as speech.
const GATE_DB: f32 = 12.0;
/// With the `selftest` feature: transcribe the built-in clip at boot.
const SELFTEST: bool = cfg!(feature = "selftest");
const COMPARE_SPECS: bool = false;
/// Spec for transcribing: the porting guide's, 5-beam search, or greedy with
/// `--features fast`.
pub const SPEC: Spec = Spec::DEFAULT;
const HINT: &str = "Hold BOOT, speak, let go";

static RECORDING: AtomicBool = AtomicBool::new(false);
static CLIP: Signal<CriticalSectionRawMutex, Vec<i16>> = Signal::new();

esp_bootloader_esp_idf::esp_app_desc!();

/// Spoken by macOS `say`, the same clip the host tests and the other boards
/// use. Only in the image with the `selftest` feature.
#[cfg(feature = "selftest")]
static SELFTEST_WAV: &[u8] = include_bytes!("../assets/selftest_en.wav");
#[cfg(not(feature = "selftest"))]
static SELFTEST_WAV: &[u8] = &[];
/// The microphone's executor. The DMA ring holds only ~125 ms, and on the
/// thread-mode executor the helper's work units and full-screen redraws (one
/// took 330 ms while core 1 filled PSRAM) left it undrained for longer: the
/// stream overflowed, and after an overflow esp-hal's restart doesn't
/// always resume it. In an interrupt it preempts all of that.
static MIC_EXECUTOR: StaticCell<InterruptExecutor<2>> = StaticCell::new();
static CORE1_STACK: ConstStaticCell<Stack<{ 96 * 1024 }>> = ConstStaticCell::new(Stack::new());

#[allow(clippy::large_stack_frames, reason = "main holds the display")]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();
    let p = board::init();
    // Internal SRAM for the engine's small hot buffers, ahead of PSRAM.
    esp_alloc::heap_allocator!(size: 160 * 1024);
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    board::wait_for_console().await;
    info!(
        "=== Cactus Whistle on ESP32-P4X-Function-EV-Board ({}) ===",
        SPEC.name()
    );

    let psram = board::psram_heap(p.PSRAM);
    info!("PSRAM: {} KiB on the heap", psram / 1024);

    let mut screen = Screen::new();
    let framebuffers = [(); display::FRAME_BUFFERS].map(|()| display::alloc_framebuffer());
    let mut lcd = match Display::new(p.MIPI_DSI, p.VDMA_CH0, lcd_pins!(p), framebuffers).await {
        Ok(d) => Some(d),
        Err(e) => {
            warn!("LCD init failed ({e:?}); transcripts go to the log only");
            None
        }
    };

    let (model, entry) =
        match unsafe { mmap::map_flash(mmap::MODEL_FLASH_OFFSET, mmap::MODEL_PARTITION_SIZE) } {
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
        error!("  cargo flash-model p4   (in the repository root)");
        screen.status("No model in flash", ui::RED);
        screen.body("cargo flash-model p4", false);
        show(&screen, &mut lcd).await;
        halt("no model").await;
    }

    // I2S first: the ES8311 is a slave clocked from BCLK.
    let mic_spawner = MIC_EXECUTOR
        .init(InterruptExecutor::new(p.FROM_CPU_INTR2))
        .start(Priority::Priority2);
    mic_spawner.spawn(mic_task(p.I2S0, p.DMA_CH0, i2s_in_pins!(p)).unwrap());
    Timer::after(Duration::from_millis(20)).await;
    match board::i2c(p.I2C0, p.GPIO8, p.GPIO7) {
        Ok(i2c) => {
            let mut codec = audio::codec(i2c);
            let init = async {
                codec.init(&mut Delay).await?;
                codec.set_mode(Mode::Record).await?;
                codec.set_mic_gain(MicGain::from_db(MIC_GAIN_DB)).await
            };
            match init.await {
                Ok(()) => info!("ES8311 recording, mic gain {MIC_GAIN_DB} dB"),
                Err(e) => error!("ES8311 init failed: {e:?}"),
            }
        }
        Err(e) => error!("I2C init failed: {e:?}"),
    }
    let mut button = board::boot_button(p.GPIO35);

    esp_rtos::start_second_core(
        p.CPU_CTRL,
        p.FROM_CPU_INTR1,
        CORE1_STACK.take(),
        move || {
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
        },
    );
    spawner.spawn(helper_task().unwrap());

    screen.status("Loading model...", ui::GREY);
    show(&screen, &mut lcd).await;

    let mut ready = false;
    let mut busy = SELFTEST; // with the self-test, it runs first

    loop {
        let msg = match select(FROM_INFERENCE.receive(), button.wait_for_falling_edge()).await {
            Either::First(m) => m,
            Either::Second(()) if ready && !busy => {
                record(&mut button, &mut screen, &mut lcd, &mut busy).await;
                continue;
            }
            Either::Second(()) => continue,
        };
        match msg {
            Msg::Status(s) => screen.status(s, ui::BLUE),
            Msg::Ready { load_ms } => {
                info!("model ready in {load_ms} ms");
                screen.status("Model ready", ui::GREY);
                ready = true;
            }
            Msg::NoSpeech => {
                busy = false;
                screen.status("No speech heard", ui::AMBER);
            }
            Msg::Result(r) => {
                busy = false;
                info!(
                    "{} [{}] {} | audio {} ms, encode {} ms, decode {} ms ({} tokens), enc_fnv {:08x}",
                    if r.selftest { "self-test" } else { "heard" },
                    r.lang,
                    r.text,
                    r.audio_ms,
                    r.enc_ms,
                    r.dec_ms,
                    r.tokens,
                    r.fnv,
                );
                screen.status(
                    if r.selftest {
                        "Self-test (built-in English clip)"
                    } else {
                        "Heard"
                    },
                    ui::GREEN,
                );
                screen.body(&r.text, r.lang == "pl");
                screen.footer(
                    &format!(
                        "language {}  |  {:.1} s of audio  |  encode {:.1} s  |  decode {:.1} s  |  {} tokens",
                        r.lang,
                        r.audio_ms as f32 / 1000.0,
                        r.enc_ms as f32 / 1000.0,
                        r.dec_ms as f32 / 1000.0,
                        r.tokens
                    ),
                    &format!(
                        "{}  |  PIE on both cores, core 0 helping  |  encoder checksum {:08x}",
                        SPEC.name(),
                        r.fnv
                    ),
                );
            }
            Msg::Error(e) => {
                busy = false;
                error!("{e}");
                screen.status("Error", ui::RED);
                screen.body(&e, false);
            }
        }
        screen.hint(if ready && !busy { HINT } else { "" });
        show(&screen, &mut lcd).await;
    }
}

/// Records while BOOT is held (at most 10 s) and hands the clip to the
/// inference core.
async fn record(
    button: &mut Input<'static>,
    screen: &mut Screen,
    lcd: &mut Option<Display<'static>>,
    busy: &mut bool,
) {
    RECORDING.store(true, Ordering::Release);
    let t0 = embassy_time::Instant::now();
    screen.status("Listening...  let go when done", ui::GREEN);
    screen.hint("");
    show(screen, lcd).await;
    // Contact bounce right after the press.
    Timer::after(Duration::from_millis(20)).await;
    let _ = select(
        button.wait_for_high(),
        Timer::after(Duration::from_secs(10)),
    )
    .await;
    RECORDING.store(false, Ordering::Release);
    let mic = CLIP.wait().await;
    info!(
        "recorded {} ms: {:.1} dBFS rms / {:.1} peak",
        t0.elapsed().as_millis(),
        rms_db(&mic),
        peak_db(&mic),
    );
    if let Some(x) = wav::prepare(&mic) {
        TO_INFERENCE.signal(x);
        *busy = true;
        screen.status("Thinking...", ui::BLUE);
    } else {
        screen.status("Too short", ui::AMBER);
        screen.hint(HINT);
    }
    show(screen, lcd).await;
}

async fn show(screen: &Screen, lcd: &mut Option<Display<'static>>) {
    if let Some(lcd) = lcd {
        screen.show(lcd).await;
    }
}

/// Core 0's share of the row work. Core 0 has PIE too, so it takes any row
/// (whistle_common::inference::helper).
#[embassy_executor::task]
async fn helper_task() {
    inference::helper(Int8Help::Full).await
}

/// Reads the microphone forever (the DMA stops if the ring overflows), in
/// the interrupt executor ([`MIC_EXECUTOR`]; no float code here: the
/// interrupt entry doesn't save the FPU registers), halves the rate, and keeps the samples while [`RECORDING`] is set; the
/// clip goes out through [`CLIP`].
#[embassy_executor::task]
async fn mic_task(i2s: I2S0<'static>, dma: DMA_CH0<'static>, pins: I2sInPins<'static>) {
    // Built here: the async driver isn't Send, so it can't be handed over
    // from the thread-mode executor.
    let mut input = match AudioIn::new(i2s, dma, pins, MIC_RATE) {
        Ok(input) => input,
        Err(e) => return error!("I2S init failed: {e:?}"),
    };
    let mut mic: Vec<i16> = Vec::new();
    let mut dec = Decimator::new();
    let mut was_recording = false;
    loop {
        let rec = RECORDING.load(Ordering::Acquire);
        if rec && !was_recording {
            mic = Vec::with_capacity(MAX_CLIP);
        }
        let result = input
            .read(|s| {
                if let Some(y) = dec.push(s)
                    && rec
                    && mic.len() < MAX_CLIP
                {
                    mic.push(y);
                }
            })
            .await;
        if was_recording && !rec {
            CLIP.signal(core::mem::take(&mut mic));
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
