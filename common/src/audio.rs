//! Microphone samples and WAV files.

use alloc::vec::Vec;

use crate::SAMPLE_RATE;

/// Recordings stop after this many samples (10 s).
pub const MAX_CLIP: usize = 10 * SAMPLE_RATE as usize;
/// Samples dropped from the start of a recording: the click of the button
/// or pad being pressed.
pub const SKIP_START: usize = SAMPLE_RATE as usize / 10;
/// Shortest recording worth transcribing, after [`SKIP_START`].
pub const MIN_CLIP: usize = SAMPLE_RATE as usize / 4;

/// A recording ready for Whistle: [`SKIP_START`] dropped, DC offset
/// removed, scaled to [-1, 1]. `None` if it is too short.
pub fn prepare(clip: &[i16]) -> Option<Vec<f32>> {
    if clip.len() <= SKIP_START + MIN_CLIP {
        return None;
    }
    Some(to_float(&clip[SKIP_START..]))
}

/// 16-bit samples to floats in [-1, 1] with the DC offset removed.
pub fn to_float(x: &[i16]) -> Vec<f32> {
    let mean = x.iter().map(|&v| v as f32).sum::<f32>() / x.len().max(1) as f32;
    x.iter().map(|&v| (v as f32 - mean) / 32768.0).collect()
}

/// RMS level in dBFS, DC removed.
pub fn rms_db(x: &[i16]) -> f32 {
    if x.is_empty() {
        return -120.0;
    }
    let mean = x.iter().map(|&v| v as f32).sum::<f32>() / x.len() as f32;
    let e = x.iter().map(|&v| (v as f32 - mean) * (v as f32 - mean)).sum::<f32>() / x.len() as f32;
    20.0 * libm::log10f(libm::sqrtf(e).max(1.0) / 32768.0)
}

/// Peak level in dBFS.
pub fn peak_db(x: &[i16]) -> f32 {
    let p = x.iter().map(|&v| (v as i32).unsigned_abs()).max().unwrap_or(0);
    20.0 * libm::log10f((p as f32).max(1.0) / 32768.0)
}

/// The samples of a 16 kHz mono 16-bit PCM WAV, as floats in [-1, 1].
pub fn parse_wav(b: &[u8]) -> Option<Vec<f32>> {
    let rd32 = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize;
    let mut off = 12;
    while off + 8 <= b.len() {
        let len = rd32(off + 4);
        if &b[off..off + 4] == b"data" {
            let d = &b[off + 8..(off + 8 + len).min(b.len())];
            return Some(d.as_chunks::<2>().0.iter().map(|&c| i16::from_le_bytes(c) as f32 / 32768.0).collect());
        }
        off += 8 + len + (len & 1);
    }
    None
}
