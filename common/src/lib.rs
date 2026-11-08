//! What the whistle-esp firmwares share. Nothing here knows the board: each
//! firmware brings up its display, microphones and buttons, starts
//! [`inference::inference_core`] on the core with PIE and
//! [`inference::helper`] on the other, and talks to them through
//! [`inference::TO_INFERENCE`] and [`inference::FROM_INFERENCE`].

#![no_std]

extern crate alloc;

pub mod audio;
pub mod inference;
pub mod text;

/// Whistle's input rate.
pub const SAMPLE_RATE: u32 = 16_000;
