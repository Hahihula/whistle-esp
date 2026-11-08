//! The two cores' jobs.
//!
//! The inference core owns the model and blocks while it transcribes; it
//! never awaits. The other core runs Embassy (UI, microphones) plus
//! [`helper`], which takes matmul rows off the inference core in short bursts
//! (`cactus_whistle::par`), so the microphone ring and the display keep being
//! served in between.

use alloc::{boxed::Box, format, string::String, vec::Vec};
use core::sync::atomic::{AtomicBool, Ordering};

use cactus_whistle::{LANGS, Spec, Whistle, par::Int8Help};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal};
use embassy_time::{Duration, Instant, Timer, block_for};
use log::info;

use crate::audio::parse_wav;

/// Recordings for the inference core, as [`crate::audio::prepare`] returns them.
pub static TO_INFERENCE: Signal<CriticalSectionRawMutex, Vec<f32>> = Signal::new();
/// Messages from the inference core to the UI.
pub static FROM_INFERENCE: Channel<CriticalSectionRawMutex, Msg, 4> = Channel::new();
/// Set while the inference core transcribes; [`helper`] works only then.
static BUSY: AtomicBool = AtomicBool::new(false);

pub enum Msg {
    Status(&'static str),
    Ready { load_ms: u32 },
    Result(Transcript),
    NoSpeech,
    Error(String),
}

pub struct Transcript {
    pub text: String,
    pub lang: &'static str,
    pub audio_ms: u32,
    pub enc_ms: u32,
    pub dec_ms: u32,
    pub tokens: usize,
    /// FNV-1a of the encoder output; `whistle transcribe` on a PC prints the
    /// same value for the same clip.
    pub fnv: u32,
    pub selftest: bool,
}

pub struct Config {
    /// Behaviour for recordings (the self-test always runs `Spec::DEFAULT`).
    pub spec: Spec,
    /// 16 kHz mono WAV transcribed at boot as a self-test (the firmwares'
    /// `selftest` feature), or `None` to go straight to recordings.
    pub selftest_wav: Option<&'static [u8]>,
    /// Run the self-test under each spec (README: measurements).
    pub compare_specs: bool,
    /// Speech must be this many dB above the quietest part of a recording
    /// (`cactus_whistle::find_speech`).
    pub gate_db: f32,
    /// Free heap in bytes, for the log.
    pub heap_free: fn() -> usize,
    /// Called once the model is open (micro-benchmarks).
    pub after_open: Option<AfterOpen>,
}

/// Gets the model and the output head's PSRAM copy.
pub type AfterOpen = fn(model: &'static [u8], head: &'static [u8]);

/// The inference core's loop. Enables PIE and checks it, opens the model,
/// copies the output head to PSRAM, transcribes the self-test clip, then
/// every recording sent through [`TO_INFERENCE`]. Returns only on an error,
/// after reporting it.
pub fn inference_core(model: &'static [u8], cfg: Config) {
    cactus_whistle::prof::set_clock(|| Instant::now().as_micros() as u32);
    // The inference core must have PIE; anything but a PIE pass means the
    // engine would run on the slow fallback, or on a broken PIE.
    esp_pie::enable();
    match esp_pie::self_test() {
        esp_pie::SelfTest::Pie => {}
        esp_pie::SelfTest::FallbackOnly => {
            return send(Msg::Error(String::from("no PIE on the inference core")));
        }
        esp_pie::SelfTest::Failed => return send(Msg::Error(String::from("PIE self-test failed"))),
    }
    // The engine's int8 dot products on PIE, on every core that has it (on
    // the S31, core 0 keeps the engine's portable loop).
    cactus_whistle::quant::set_kernel(Some(cactus_whistle::quant::Kernel {
        dot_rows: pie_dot_rows,
        here: esp_pie::available,
    }));
    info!("inference core: PIE enabled and checked");

    let t0 = Instant::now();
    let mut w = match Whistle::open(model) {
        Ok(w) => w,
        Err(e) => return send(Msg::Error(format!("model: {e:?}"))),
    };
    // The tied output head (8199 rows) is read in full for every token: a
    // copy in PSRAM streams faster than the flash mapping.
    let head: &'static [u8] = Box::leak(w.emb_bytes().to_vec().into_boxed_slice());
    w.set_emb_bytes(head);
    info!("model open: {} decoder layers, head {} KiB copied to PSRAM", w.depth, head.len() / 1024);
    send(Msg::Ready { load_ms: t0.elapsed().as_millis() as u32 });
    if let Some(f) = cfg.after_open {
        f(model, head);
    }

    if let Some(wav) = cfg.selftest_wav {
        send(Msg::Status("Self-test..."));
        match parse_wav(wav) {
            Some(x) if cfg.compare_specs => compare_specs(&mut w, &x, &cfg),
            Some(x) => run(&mut w, &x, true, &cfg),
            None => send(Msg::Error(String::from("bad self-test WAV"))),
        }
    }
    w.set_spec(cfg.spec);

    loop {
        let Some(x) = TO_INFERENCE.try_take() else {
            block_for(Duration::from_millis(10));
            continue;
        };
        match cactus_whistle::find_speech(&x, 0.0, cfg.gate_db, 300) {
            Some((s0, s1)) => run(&mut w, &x[s0..s1], false, &cfg),
            None => send(Msg::NoSpeech),
        }
    }
}

/// `cactus_whistle::quant::Kernel::dot_rows` on PIE: one 128-lane dot per
/// group, for each activation row.
///
/// # Safety
///
/// As the engine guarantees: `w` holds `128 * groups` and `x`
/// `128 * groups * rows` bytes, both 16-byte aligned; called only where
/// `esp_pie::available()`, with PIE enabled on that core.
unsafe fn pie_dot_rows(w: *const i8, x: *const i8, groups: usize, rows: usize, out: &mut [i32]) {
    // Plain pointer stepping: this runs once per weight row, and in the
    // decoder (5 activation rows) its loop overhead shows in the timings.
    let mut o = out.as_mut_ptr();
    let mut xg = x;
    for _ in 0..rows {
        let mut wg = w;
        for _ in 0..groups {
            // SAFETY: forwarded from the engine's contract above; `o` stays
            // within `out` (rows * groups entries).
            unsafe {
                *o = esp_pie::dot_i8x128_unchecked(wg, xg);
                o = o.add(1);
                wg = wg.add(128);
                xg = xg.add(128);
            }
        }
    }
}

/// The other core's share of the work, as an Embassy task body. `int8`
/// says which int8 matmul rows it may take: `Full` if this core has PIE too
/// (ESP32-S3), `Light` if not (ESP32-S31 core 0: only the decoder's
/// unpack-bound rows, with the plain dot product). Works in bursts of at
/// most 15 ms.
pub async fn helper(int8: Int8Help) -> ! {
    esp_pie::enable(); // does nothing on a core without PIE
    cactus_whistle::par::set_helper_int8(int8);
    cactus_whistle::par::set_helper(true);
    loop {
        if BUSY.load(Ordering::Acquire) {
            let t0 = Instant::now();
            while cactus_whistle::par::help() && t0.elapsed() < Duration::from_millis(15) {}
            embassy_futures::yield_now().await;
        } else {
            Timer::after(Duration::from_millis(5)).await;
        }
    }
}

/// Queues a message for the UI, waiting for room rather than dropping it.
fn send(msg: Msg) {
    let mut m = msg;
    while let Err(embassy_sync::channel::TrySendError::Full(back)) = FROM_INFERENCE.try_send(m) {
        m = back;
        block_for(Duration::from_millis(10));
    }
}

/// The self-test under the C engine's behaviour and the porting guide's,
/// greedy and with the 5-beam search.
fn compare_specs(w: &mut Whistle, x: &[f32], cfg: &Config) {
    for spec in [Spec::CEngine, Spec::GuideFast, Spec::Guide] {
        w.set_spec(spec);
        info!("== self-test: {}", spec.name());
        run(w, x, true, cfg);
    }
}

/// Encodes and decodes one clip and reports it.
fn run(w: &mut Whistle, x: &[f32], selftest: bool, cfg: &Config) {
    cactus_whistle::prof::reset();
    BUSY.store(true, Ordering::Release);
    let t0 = Instant::now();
    let (enc, t) = match w.encode(x) {
        Ok(v) => v,
        Err(e) => {
            BUSY.store(false, Ordering::Release);
            return send(Msg::Error(format!("encode: {e:?}")));
        }
    };
    let enc_ms = t0.elapsed().as_millis() as u32;
    let fnv = enc.iter().fold(0x811C_9DC5u32, |h, v| (h ^ v.to_bits()).wrapping_mul(0x0100_0193));
    let t1 = Instant::now();
    let ids = w.decode(&enc, t, None, 320);
    let dec_ms = t1.elapsed().as_millis() as u32;
    BUSY.store(false, Ordering::Release);
    for (name, v) in cactus_whistle::prof::NAMES.iter().zip(cactus_whistle::prof::read()) {
        info!("  prof {name:<9} {:>7} ms", v / 1000);
    }
    info!("  heap free {} KiB", (cfg.heap_free)() / 1024);
    send(Msg::Result(Transcript {
        text: w.detok(&ids),
        lang: LANGS[w.lang],
        audio_ms: (x.len() / 16) as u32,
        enc_ms,
        dec_ms,
        tokens: ids.len(),
        fnv,
        selftest,
    }));
}
