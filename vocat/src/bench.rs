//! On-board micro-benchmark of the int8 matmul kernel (`BENCH` in main.rs):
//! the same weight rows streamed from flash, from PSRAM and from internal
//! SRAM, for a 2-bit decoder matrix and the 4-bit output head, T = 1.

use alloc::vec;
use alloc::vec::Vec;

use log::info;
use cactus_whistle::{
    cact::{Cact, Tensor},
    quant::{Q8Act, q8_gemm},
};

fn cycles() -> u32 {
    let c: u32;
    unsafe { core::arch::asm!("rsr.ccount {0}", out(reg) c) };
    c
}

/// Cycles per output row of `q8_gemm` over the first `rows` rows.
fn per_row(c: &Cact, t: &Tensor, blob: &[u8], act: &Q8Act, rows: usize, reps: usize) -> u32 {
    let mut y = vec![0f32; rows];
    q8_gemm(c, t, blob, act, 0, rows, &mut y); // warm tables and code
    let t0 = cycles();
    for _ in 0..reps {
        q8_gemm(c, t, blob, act, 0, rows, &mut y);
    }
    t0.wrapping_sub(cycles()).wrapping_neg() / (reps * rows) as u32
}

/// A copy of the first `rows` packed rows plus *all* norms laid out where
/// `q8_gemm` expects them (norms follow `shape[0]` rows), so only the rows
/// we touch need to be real.
fn copy_rows(t: &Tensor, blob: &[u8], rows: usize) -> (Tensor, Vec<u8>) {
    let rowb = t.row_bytes();
    let ng = t.groups();
    let mut t2 = *t;
    t2.shape[0] = rows as u32;
    let mut v = Vec::with_capacity(rows * rowb + rows * ng * 2);
    v.extend_from_slice(&blob[..rows * rowb]);
    let norms = t.shape[0] as usize * rowb;
    v.extend_from_slice(&blob[norms..norms + rows * ng * 2]);
    (t2, v)
}

/// Cycles per call of `f`, averaged over `reps`.
fn time(reps: u32, mut f: impl FnMut()) -> u32 {
    f();
    let t0 = cycles();
    for _ in 0..reps {
        f();
    }
    cycles().wrapping_sub(t0) / reps
}

/// Checks that madd.s (via cactus_whistle::math::fma) is a true fused multiply-add,
/// i.e. matches the correctly rounded libm::fmaf; otherwise host and board
/// would round differently.
fn fma_check() -> (u32, u32) {
    let mut seed = 0x2468_ACE1u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        // finite floats in a wide range of magnitudes
        f32::from_bits((seed & 0x807F_FFFF) | ((seed >> 8 & 0x3F) + 100) << 23)
    };
    let (mut diff, n) = (0u32, 20_000u32);
    for _ in 0..n {
        let (a, b, c) = (next(), next(), next());
        if cactus_whistle::math::fma(a, b, c).to_bits() != libm::fmaf(a, b, c).to_bits() {
            diff += 1;
        }
    }
    (diff, n)
}

/// The float kernels outside the matmul.
pub fn kernels(model: &[u8]) {
    let c = Cact::open(model).expect("model");
    let gate = c.tensor(10).expect("tensor"); // 512 wide
    let x512: Vec<f32> = (0..512).map(|i| ((i * 37 % 101) as f32 - 50.0) / 50.0).collect();
    let x31: Vec<f32> = (0..31 * 512).map(|i| ((i * 37 % 101) as f32 - 50.0) / 50.0).collect();
    info!(
        "bench Q8Act::prepare: 1x512 {} cyc/group, 31x512 {} cyc/group",
        time(50, || drop(core::hint::black_box(Q8Act::prepare(&gate, &x512, 1)))) / 4,
        time(5, || drop(core::hint::black_box(Q8Act::prepare(&gate, &x31, 31)))) / (31 * 4)
    );
    let mut blk = [0.5f32; 128];
    info!(
        "bench fwht(128): {} cyc",
        time(100, || cactus_whistle::math::fwht(core::hint::black_box(&mut blk[..])))
    );
    let z = [0.25f32; 512];
    let a = [0.5f32; 256];
    let b = [0.125f32; 1024];
    let mut o = [0f32; 512];
    info!(
        "bench kron 16x32 (24.6K MAC): {} cyc",
        time(20, || cactus_whistle::bench::kron(core::hint::black_box(&z), &a, &b, &mut o))
    );
    let (d, n) = fma_check();
    info!("bench madd.s vs libm::fmaf: {d} of {n} differ");
}

pub fn run(model: &[u8], head_psram: &[u8]) {
    kernels(model);
    let c = Cact::open(model).expect("model");
    let x: Vec<f32> = (0..512).map(|i| ((i * 37 % 101) as f32 - 50.0) / 50.0).collect();
    cactus_whistle::par::set_helper(false); // single core: cycles per row on core 1

    // Decoder layer 0 gate_proj: 512 x 512, 2-bit, in flash.
    let gate = c.tensor(10).expect("tensor");
    let gate_blob = c.data(&gate).expect("data");
    let a = Q8Act::prepare(&gate, &x, 1);
    let (g2, g_sram) = copy_rows(&gate, gate_blob, 256);
    info!(
        "bench 2-bit row (512 wide): flash {} cyc, SRAM {} cyc",
        per_row(&c, &gate, gate_blob, &a, 512, 4),
        per_row(&c, &g2, &g_sram, &a, 256, 8)
    );

    // Output head: 8199 x 512, 4-bit.
    let head = c.tensor(0).expect("tensor");
    let head_flash = c.data(&head).expect("data");
    let a = Q8Act::prepare(&head, &x, 1);
    let (h2, h_sram) = copy_rows(&head, head_flash, 128);
    info!(
        "bench 4-bit row (512 wide): flash {} cyc, PSRAM {} cyc, SRAM {} cyc",
        per_row(&c, &head, head_flash, &a, 2048, 1),
        per_row(&c, &head, head_psram, &a, 2048, 1),
        per_row(&c, &h2, &h_sram, &a, 128, 8)
    );
    cactus_whistle::par::set_helper(true);
}
