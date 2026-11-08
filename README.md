# whistle-esp

Offline speech recognition on Espressif microcontrollers: hold a button, speak, and
the transcript appears on the board's screen. Everything runs on the chip. There is
no network, no operating system, and it's all `no_std` Rust.

It runs [Cactus Whistle](https://huggingface.co/Cactus-Compute/whistle), Cactus
Compute's multilingual speech model (English, German, French, Spanish, Italian, Dutch,
Polish, with language detection), on the
[cactus-whistle-rs](https://github.com/Hahihula/cactus-whistle-rs) engine.

**Try it without installing anything:** flash the firmware and the model straight
from your browser at **[hahihula.github.io/whistle-esp](https://hahihula.github.io/whistle-esp/)**
(Chrome or Edge on a desktop; it uses Web Serial).

| board | chip | screen | push to talk | 2.5 s clip, 5-beam / greedy |
|---|---|---|---|---|
| [ESP-VoCat v1.2](https://github.com/Hahihula/esp-vocat-rust-examples) | ESP32-S3, 2x Xtensa LX7 + PIE | 360x360 round | either touch pad on the cat's head | 16.5 s / 10.6 s |
| [ESP32-S31-Korvo-1](https://github.com/Hahihula/s31-korvo-1-rust-examples) | ESP32-S31, 2x RISC-V, PIE on core 1 | 800x480 | any of the four buttons | 20.0 s / 13.6 s |

## Quick start

The quickest way is the [web flasher](https://hahihula.github.io/whistle-esp/): pick
the board, plug it in, click flash. To build from source instead, you need:

- [`espflash`](https://github.com/esp-rs/espflash) 4.x;
- for the VoCat, the Xtensa Rust toolchain: `cargo install espup && espup install`;
- for the Korvo, stable Rust (the toolchain file adds `rust-src` and the target).

Then, in the repository root:

```bash
cargo xtask setup vocat     # or: cargo xtask setup korvo
```

This downloads the model (16.9 MB) from Hugging Face and trims it to 4 of its 8
decoder layers (14.5 MB, so it fits the 16 MB flash). Then it writes the model into
its flash partition, builds and flashes the firmware, and opens the serial monitor.
Later runs skip the download and the trim. On the Korvo use the **UART** USB port.

The board is ready about a second after boot: hold a pad or button, speak (up to
10 s) and let go.

## Commands

All from the repository root:

| command | what |
|---|---|
| `cargo fetch-model` | download `model/whistle.cact` |
| `cargo trim-model [DEPTH]` | write `model/whistle_d4.cact` (or another depth) |
| `cargo flash-model vocat\|korvo` | write the trimmed model into the board's `model` partition (once; it survives firmware updates) |
| `cargo run-vocat`, `cargo run-korvo` | build, flash and monitor the firmware |
| `cargo build-vocat`, `cargo build-korvo` | build only |

Options:
- `--fast` (build/run) switches to greedy decoding, which roughly halves decoding
  time for about 3 points more word error rate.
- `--selftest` (build/run) makes the firmware transcribe a built-in English clip at
  boot and log its timings and encoder checksum, before taking recordings. It's for
  development and automated checks; it keeps the board busy for about 16-20 s.
- `--splash` (build/run) plays the boot splash (see below). It is off by default: a
  production firmware goes straight to the user interface, without the animation, its
  marks and the couple of seconds it takes.
- `--port PORT` picks the serial port when several boards are connected.
- `--depth N` (flash-model) flashes another depth, if it fits.

These are shortcuts for `cargo xtask ...`; `cargo xtask help` lists everything.

## Releases

CI (`.github/workflows/ci.yml`) checks formatting, runs clippy and builds both
firmwares on every push. Pushing a `v*` tag runs `release.yml`, which:
- builds the flash images for both boards (bootloader, partition table and app, from
  address 0, without touching the model partition);
- trims the model;
- attaches the images and the model to the GitHub release;
- publishes the web flasher (`site/`) with those files on GitHub Pages.

The flasher uses [esptool-js](https://github.com/espressif/esptool-js).

`cargo xtask` needs no Xtensa environment in your shell: if the Xtensa linker isn't
on `PATH`, it loads `~/export-esp.sh` for the VoCat build itself.

## Layout

| path | what |
|---|---|
| `vocat/` | ESP-VoCat firmware: board bring-up, touch pads, round-screen UI, flash mapping and the runtime QIO switch |
| `korvo/` | ESP32-S31-Korvo-1 firmware: board bring-up, buttons, 800x480 UI, flash mapping |
| `common/` | `whistle-common`, the board-independent half: the inference core's loop, the helper core's task, the message channel, audio and text helpers, the boot splash |
| `xtask/` | the host commands above |
| `tools/` | the splash asset generator and a host preview of the splash |
| `assets/brand/` | the brand artwork the splash is cut from |
| `model/` | downloaded and trimmed models (not in git) |

The firmwares are separate Cargo packages, because each builds for its own chip with
its own toolchain. `common/` and `xtask/` form the host workspace. Board support comes
from the [esp-vocat](https://github.com/Hahihula/esp-vocat-rust-examples) and
[esp32-s31-korvo](https://github.com/Hahihula/s31-korvo-1-rust-examples) libraries,
and the SIMD kernels from [esp-pie](https://github.com/Hahihula/esp-pie).

## How it works

Both firmwares split the work the same way:

- **The core with PIE runs Whistle** and blocks while it transcribes:
  `whistle_common::inference::inference_core`. Core 1 on both boards, the only one
  with PIE on the S31. It enables PIE and checks it against plain integer math, opens
  the model in place from flash, and copies the output head (8199 rows, read in full
  for every token) to PSRAM. Then it waits for recordings (with `--selftest`, after
  transcribing the built-in clip).
- **The other core runs Embassy:** microphones, buttons, the screen, and
  `inference::helper`, which takes matmul rows, convolutions and attention heads off
  the inference core in bursts of at most 15 ms. The microphone DMA ring holds only
  about 128 ms, so it has to keep being drained. On the S3 the helper has PIE too and
  takes any row. On the S31 it takes float work and the decoder's small int8 matmuls,
  using the plain dot product.
- **The model** sits in its own 14 MB flash partition (`partitions.csv`) and is mapped
  into the address space, so the engine reads weights in place. The S3 maps it with
  the ROM's MMU routine into the data-bus window after PSRAM (`vocat/src/accel.rs`).
  The S31 has a separate flash MMU and a 256 MB window, and the firmware writes the
  MMU entries itself (`korvo/src/mmap.rs`).
- **Results don't depend on the board.** The self-test (`--selftest`) logs a checksum of the encoder
  output (`enc_fnv`). It is the same on both boards and on a PC (`whistle transcribe`
  in cactus-whistle-rs): `db9a1371` for the default spec.

## Boot splash

The splash is behind the `splash` feature (`cargo run-vocat --splash`,
`cargo run-korvo --splash`); without it the firmware is built without the
animation at all.

Both firmwares open with the same short animation on a black screen: the
Espressif mark, a heart and the Cactus Compute mark in one line in the middle
of the screen. The line fades in, the two marks travel apart vertically, and
the names "espressif" and "cactuscompute" slide in next to them: espressif
loves cactuscompute. The two rows are the same width and mirror each other
around the heart, so the whole composition is centred on the screen.

The animation is `whistle_common::splash` (in `common/src/splash.rs`); each
firmware only runs the loop and flushes its own screen. The marks are white
line art, so the firmware only needs their coverage, one byte per pixel:
`tools/make_splash_assets.py` cuts them out of the artwork in `assets/brand/`
(the Espressif press kit and the Cactus Compute brand page) and writes the
masks into `common/assets/`. The heart is drawn from the same script.

The firmware tints the coverage: the Espressif mark is red, the Cactus Compute
mark and both names are white, the heart is rose. The colours are set per board
in `Splash::vocat()` and `Splash::korvo()`.

Only the areas the last frame drew are erased and redrawn, so a frame touches
the rows the animation is on instead of the whole screen, which matters on the
Korvo, where the LCD is scanned out of PSRAM. What has come to a stand-still
(the heart after it has faded in, the marks after they have split, the names
after they have slid in) is drawn one last time and then left alone, so the
later frames cost nothing.

On the Korvo the splash plays *before* the model is loaded on the second
core. The load copies a few megabytes of the model's output head through
PSRAM, and PSRAM has little headroom there: the LCD's scan-out falls behind,
the picture drifts and the driver has to realign it with a blink. That costs
about 2.6 s of boot time on that board.

To see the splash without a board, `tools/splash-preview` renders it on the
host, with the very code the firmwares run:

```bash
cd tools/splash-preview && cargo run --release   # writes /tmp/splash_{vocat,korvo}.ppm
```

## Speed

Self-test (`--selftest`), a 2.5 s English clip (15 tokens), depth-4 model:

| | VoCat (S3) encode / decode | Korvo (S31) encode / decode |
|---|---|---|
| default: porting-guide front end, 5-beam search | 5.8 / 10.7 s | 5.8 / 14.2 s |
| `--fast`: the same, greedy decoding | 5.8 / 4.8 s | 5.9 / 7.7 s |

Decoding with five beams costs 2.3x (VoCat) and 1.8x (Korvo) the greedy time, because
all live beams go through each decoder step as one batch. Accuracy against Cactus's own
engine is measured in cactus-whistle-rs: 31.6% word error rate for the default spec,
34.8% greedy, 31.3% for the official engine on the same evaluation set.

How the VoCat got from 21.4 s to 10.6 s (greedy, C engine's behaviour):

| step | encode | decode | total |
|---|---|---|---|
| first run, one core | 10.5 s | 10.9 s | 21.4 s |
| + row work shared with core 0 | 9.0 s | 7.4 s | 16.3 s |
| + flash in QIO | 8.9 s | 6.9 s | 15.8 s |
| + faster row unpack in the int8 matmul | 8.8 s | 5.4 s | 14.1 s |
| + register-blocked float kernels, 3-pass Hadamard | 8.4 s | 5.3 s | 13.6 s |
| + fused multiply-add, tighter int8 activation prep | 7.4 s | 5.0 s | 12.5 s |
| + aligned fp16 views for norms and MLP weights | 7.2 s | 4.7 s | 11.9 s |
| + stem, attention, int8 prep and mHC mixing split across cores | 5.9 s | 4.7 s | **10.6 s** |

And the Korvo (same behaviour):

| step | encode | decode | total |
|---|---|---|---|
| one core, plain int8 dot product | 23.9 s | 17.7 s | 41.6 s |
| + PIE on core 1 | 6.6 s | 8.2 s | 14.8 s |
| + core 0 on float rows | 5.4 s | 8.2 s | 13.6 s |
| + core 0 on decoder int8 rows (plain dot product) | 5.5 s | 7.4 s | **12.9 s** |

## Board notes

### ESP-VoCat (ESP32-S3)

- **Flash in QIO, switched at run time** (`vocat/src/qio.rs`). The firmware is flashed
  as DIO at 80 MHz, because `--flash-mode qio` goes into the bootloader header too and
  the S3 ROM then can't load the bootloader (boot loop, `ets_loader.c 78`). The app
  does what ESP-IDF's bootloader does:
  - checks the QE bit (already set on this board's GD25LQ128);
  - routes the WP#/HOLD# pads (GPIO28/27) to the flash controller;
  - sets SPI0 to opcode `0xEB` with 6 dummy clocks.

  It then re-reads a checksummed block and stays in DIO on a mismatch. The ROM's
  `esp_rom_spiflash_config_readmode` alone hangs the app, because the cache keeps
  sending the DIO opcode, and without the pad routing IO2/IO3 read high.
- **Thin LTO.** Fat LTO trips an Xtensa LLVM backend bug in esp-hal's DMA code once
  the engine is linked in.
- The BOOT button is inside the case on the devkit, so recording uses the touch
  pads. MIC1 of the ES7210 is transcribed, at 30 dB gain.
- `BENCH` in `vocat/src/main.rs` runs a matmul micro-benchmark at boot. Flash, PSRAM
  and internal SRAM come within 10-20% of each other; the cost is unpacking the 2/4-bit
  weights, not reading them.

### ESP32-S31-Korvo-1

- **PIE only on core 1.** Core 0 raises an illegal-instruction exception on the first
  vector instruction even with PIE enabled. ESP-IDF's FreeRTOS port says the same
  (`portasm.S`).
- **Screen bandwidth.** The 800x480 panel is refreshed continuously by DMA from a
  frame buffer in PSRAM. The firmware runs it at a 15 MHz pixel clock (~29.5 frames/s,
  ~30 of the ~55 MB/s PSRAM gives DMA) instead of the driver's 18 MHz (~35 frames/s,
  ~36 MB/s), which makes the inference about 2% faster. The panel's floor is above
  12 MHz: there it shows its own colour-bar test pattern. The output head still reads
  faster from PSRAM than from the (DIO) flash: decoding takes 7.5 s with it in PSRAM
  against 10.5 s in flash.

  While the inference runs, its PSRAM traffic makes the scan-out fall behind about
  2-3 times a second, so the picture drifts sideways until the transcript is drawn
  and the driver realigns the stream. Realigning it during inference too (every
  200 ms) keeps it still but blinks 2-3 times a second, which looks worse. Giving the
  LCD's DMA read priority in the chip's interconnect (the `ICM_SYS` QoS registers)
  should stop the drift at some cost to inference; it hasn't been tried.
- **Microphones.** The ES8389 codec is set up like Espressif's Korvo-1 BSP: 16-bit
  Philips I2S, no MCLK, codec as slave. The one difference is that both ADC channels
  are microphones, with no DAC echo-reference channel. MIC1 is transcribed, at 33.5 dB
  gain (24.5 dB peaked at only -24 dBFS).
- Not tried yet: a runtime QIO switch for the S31's flash.

### Settings

At the top of each `main.rs`:
- `COMPARE_SPECS`, with `--selftest`, runs the self-test under every spec and logs
  the times.
- `MIC_GAIN_DB` sets the microphone gain.
- `GATE_DB` sets the speech gate: how far above the recording's quietest part speech
  must be.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.

The Whistle model is not part of this repository. `cargo fetch-model` downloads it
from Cactus Compute's model page, where its own license applies (Apache-2.0 at the
time of writing).

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions.

## Acknowledgements

- [Cactus Compute](https://cactuscompute.com) made Whistle, published the
  [model](https://huggingface.co/Cactus-Compute/whistle) and the
  [porting guide](https://cactuscompute.com/blog/porting-whistle) that the engine's
  default behaviour follows.
- [andrisgauracs/whistle-esp32](https://github.com/andrisgauracs/whistle-esp32) was the
  inspiration for this project. It's a C99 Whistle engine and ESP32-S3 firmware, and
  the Rust engine started as a port of it. Mapping the model partition into the S3's
  data bus and splitting the work across both cores follow its approach.
- Espressif's [ESP-IDF](https://github.com/espressif/esp-idf) and `esp_codec_dev`
  documented the PIE, flash MMU, QIO and codec details the firmwares rely on, and
  [esp-hal](https://github.com/esp-rs/esp-hal) is what the firmwares are built on.
