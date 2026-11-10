//! 32 kHz to 16 kHz. The ES8311 driver runs from the I2S bit clock and
//! supports 22.05 kHz and up, so the microphone records at 32 kHz and this
//! halves it for Whistle.
//!
//! A 27-tap half-band FIR (Blackman-windowed sinc, cut-off 8 kHz): flat to
//! 6 kHz, -6 dB at 8 kHz, -35 dB at 10 kHz, below -78 dB from 12 kHz. Every
//! other tap of a half-band filter is zero, so only the centre and the odd
//! taps are multiplied.
//!
//! Integer only (Q15 taps): it runs in the microphone's interrupt executor,
//! and the interrupt entry doesn't save the FPU registers, so float code
//! there would corrupt the float state of the task it preempts.

/// The non-zero taps right of the centre (offsets 1, 3, 5, ... 13), Q15.
const ODD: [i32; 7] = [10244, -2953, 1314, -587, 233, -73, 13];
/// The centre tap, Q15. The taps sum to 32766 (unity gain).
const CENTRE: i32 = 16384;
const HALF: usize = 13;
const LEN: usize = 2 * HALF + 1;

pub struct Decimator {
    /// The last LEN input samples, as a ring.
    ring: [i32; LEN],
    pos: usize,
    odd: bool,
}

impl Decimator {
    pub const fn new() -> Self {
        Self {
            ring: [0; LEN],
            pos: 0,
            odd: false,
        }
    }

    /// Takes one 32 kHz sample; returns a 16 kHz sample for every second one.
    pub fn push(&mut self, x: i16) -> Option<i16> {
        self.ring[self.pos] = i32::from(x);
        self.pos = (self.pos + 1) % LEN;
        self.odd = !self.odd;
        if self.odd {
            return None;
        }
        // self.pos is now the oldest sample; the centre is HALF after it.
        let at = |k: usize| self.ring[(self.pos + k) % LEN];
        // |sum| <= 32768 * (16384 + 2 * 15417) < 2^31.
        let mut y = CENTRE * at(HALF);
        for (i, &c) in ODD.iter().enumerate() {
            let d = 2 * i + 1;
            y += c * (at(HALF - d) + at(HALF + d));
        }
        let y = (y + (1 << 14)) >> 15;
        Some(y.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16)
    }
}
