//! `cargo xtask`: host commands for whistle-esp. Run from anywhere in the
//! repository; the aliases in `.cargo/config.toml` are shortcuts.
//!
//!     cargo xtask fetch-model
//!     cargo xtask trim-model [DEPTH]
//!     cargo xtask flash-model vocat|korvo [--depth N] [--port PORT]
//!     cargo xtask build vocat|korvo [--fast] [--splash]
//!     cargo xtask run vocat|korvo [--fast] [--splash] [--port PORT]
//!     cargo xtask setup vocat|korvo [--splash] [--port PORT]

use std::{
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

const MODEL_URL: &str = "https://huggingface.co/Cactus-Compute/whistle/resolve/main/whistle.cact";
const DEFAULT_DEPTH: usize = 4;

const USAGE: &str = "\
cargo xtask <command>

  fetch-model                    download whistle.cact (16.9 MB) from Hugging Face into model/
  trim-model [DEPTH]             keep DEPTH decoder layers (default 4: 14.5 MB, fits the boards)
  flash-model BOARD [--depth N] [--port PORT]
                                 write the trimmed model into the board's model partition
  build BOARD [--fast] [--selftest] [--splash]
                                 build the firmware (--fast: greedy decoding instead of
                                 5 beams; --selftest: transcribe a built-in clip at boot;
                                 --splash: play the boot splash)
  run BOARD [--fast] [--selftest] [--splash] [--port PORT]
                                 build, flash and open the serial monitor
  setup BOARD [--splash] [--port PORT]
                                 all of the above, skipping steps already done

BOARD is vocat (ESP-VoCat, ESP32-S3) or korvo (ESP32-S31-Korvo-1).
Aliases: cargo fetch-model, trim-model, flash-model, build-vocat, run-vocat, build-korvo, run-korvo.";

#[derive(Clone, Copy)]
enum Board {
    Vocat,
    Korvo,
}

impl Board {
    fn parse(s: Option<&String>) -> Result<Self, String> {
        match s.map(String::as_str) {
            Some("vocat") => Ok(Board::Vocat),
            Some("korvo") => Ok(Board::Korvo),
            Some(other) => Err(format!("unknown board {other}: use vocat or korvo")),
            None => Err(String::from("which board? vocat or korvo")),
        }
    }
    fn dir(self) -> PathBuf {
        root().join(match self {
            Board::Vocat => "vocat",
            Board::Korvo => "korvo",
        })
    }
    fn chip(self) -> &'static str {
        match self {
            Board::Vocat => "esp32s3",
            Board::Korvo => "esp32s31",
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("fetch-model") => fetch_model(),
        Some("trim-model") => match args.get(1).map(|d| d.parse::<usize>()) {
            None => trim_model(DEFAULT_DEPTH),
            Some(Ok(d)) => trim_model(d),
            Some(Err(_)) => Err(String::from("DEPTH must be a number")),
        },
        Some("flash-model") => {
            Board::parse(args.get(1)).and_then(|b| flash_model(b, &Opts::parse(&args[2..])?))
        }
        Some("build") => Board::parse(args.get(1))
            .and_then(|b| cargo_firmware(b, "build", &Opts::parse(&args[2..])?)),
        Some("run") => Board::parse(args.get(1))
            .and_then(|b| cargo_firmware(b, "run", &Opts::parse(&args[2..])?)),
        Some("setup") => {
            Board::parse(args.get(1)).and_then(|b| setup(b, &Opts::parse(&args[2..])?))
        }
        Some("help" | "-h" | "--help") | None => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command {other}\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Default)]
struct Opts {
    fast: bool,
    selftest: bool,
    splash: bool,
    depth: Option<usize>,
    port: Option<String>,
}

impl Opts {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut o = Opts::default();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--fast" => o.fast = true,
                "--selftest" => o.selftest = true,
                "--splash" => o.splash = true,
                "--depth" => {
                    let d = it.next().ok_or("--depth needs a number")?;
                    o.depth = Some(d.parse().map_err(|_| "--depth needs a number")?);
                }
                "--port" => o.port = Some(it.next().ok_or("--port needs a serial port")?.clone()),
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(o)
    }
}

/// The repository root (xtask lives one level below it).
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn model_dir() -> PathBuf {
    root().join("model")
}

fn full_model() -> PathBuf {
    model_dir().join("whistle.cact")
}

fn trimmed_model(depth: usize) -> PathBuf {
    model_dir().join(format!("whistle_d{depth}.cact"))
}

fn fetch_model() -> Result<(), String> {
    std::fs::create_dir_all(model_dir()).map_err(|e| e.to_string())?;
    let out = full_model();
    println!("downloading {MODEL_URL}");
    let part = out.with_extension("cact.part");
    run(Command::new("curl")
        .args(["--fail", "--location", "--progress-bar", "--output"])
        .arg(&part)
        .arg(MODEL_URL))?;
    std::fs::rename(&part, &out).map_err(|e| e.to_string())?;
    // Fail early on an HTML error page or a format change.
    let blob = std::fs::read(&out).map_err(|e| e.to_string())?;
    cactus_whistle::cact::Cact::open(&blob)
        .map_err(|e| format!("{} is not a .cact v4 model: {e:?}", out.display()))?;
    println!("{} ({:.2} MB)", out.display(), blob.len() as f64 / 1e6);
    Ok(())
}

fn trim_model(depth: usize) -> Result<(), String> {
    let src = full_model();
    let blob = std::fs::read(&src)
        .map_err(|e| format!("{}: {e} (run `cargo fetch-model` first)", src.display()))?;
    let t =
        cactus_whistle::ladder::trim(&blob, depth).map_err(|e| format!("cannot trim: {e:?}"))?;
    let dst = trimmed_model(depth);
    std::fs::write(&dst, &t.blob).map_err(|e| format!("{}: {e}", dst.display()))?;
    println!(
        "depth {depth}: decoder layers {:?} -> {} ({:.2} MB)",
        t.layers,
        dst.display(),
        t.blob.len() as f64 / 1e6
    );
    Ok(())
}

/// Offset and size of the `model` partition in the board's partitions.csv,
/// so the firmware and this command can't disagree.
fn model_partition(board: Board) -> Result<(u32, u32), String> {
    let path = board.dir().join("partitions.csv");
    let csv = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let num = |s: &str| {
        let s = s.trim();
        s.strip_prefix("0x")
            .or(s.strip_prefix("0X"))
            .map_or_else(|| s.parse(), |h| u32::from_str_radix(h, 16))
    };
    for line in csv.lines() {
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        if f.first() == Some(&"model") && f.len() >= 5 {
            return Ok((
                num(f[3]).map_err(|e| e.to_string())?,
                num(f[4]).map_err(|e| e.to_string())?,
            ));
        }
    }
    Err(format!("no `model` partition in {}", path.display()))
}

fn flash_model(board: Board, o: &Opts) -> Result<(), String> {
    let depth = o.depth.unwrap_or(DEFAULT_DEPTH);
    let file = trimmed_model(depth);
    let len = std::fs::metadata(&file)
        .map_err(|e| {
            format!(
                "{}: {e} (run `cargo trim-model {depth}` first)",
                file.display()
            )
        })?
        .len();
    let (offset, size) = model_partition(board)?;
    if len > size as u64 {
        return Err(format!(
            "{} is {len} bytes, the model partition holds {size}: use a smaller depth",
            file.display()
        ));
    }
    let mut cmd = Command::new("espflash");
    cmd.args(["write-bin", "--chip", board.chip()]);
    if let Some(p) = &o.port {
        cmd.args(["--port", p]);
    }
    cmd.arg(format!("{offset:#x}")).arg(&file);
    run(&mut cmd)
}

/// `cargo build|run --release` in the firmware's directory, so its own
/// toolchain (rust-toolchain.toml), target and runner (.cargo/config.toml)
/// apply.
fn cargo_firmware(board: Board, verb: &str, o: &Opts) -> Result<(), String> {
    let mut args = vec![verb.to_string(), "--release".to_string()];
    let features: Vec<&str> = [
        (o.fast, "fast"),
        (o.selftest, "selftest"),
        (o.splash, "splash"),
    ]
    .into_iter()
    .filter_map(|(on, f)| on.then_some(f))
    .collect();
    if !features.is_empty() {
        args.extend(["--features".to_string(), features.join(",")]);
    }
    let mut cmd = match board {
        Board::Vocat if !on_path("xtensa-esp32s3-elf-gcc") => {
            // espup's environment (Xtensa linker on PATH) isn't loaded in
            // this shell: load it for the build only.
            let env = std::env::var("HOME")
                .map(|h| PathBuf::from(h).join("export-esp.sh"))
                .ok()
                .filter(|p| p.exists());
            let Some(env) = env else {
                return Err(String::from(
                    "the Xtensa linker is not on PATH: install the esp toolchain with `espup install` \
                     and run `. ~/export-esp.sh`",
                ));
            };
            let mut c = Command::new("sh");
            c.arg("-c")
                .arg(format!(". '{}' && exec cargo \"$@\"", env.display()))
                .arg("cargo")
                .args(&args);
            c
        }
        _ => {
            let mut c = Command::new("cargo");
            c.args(&args);
            c
        }
    };
    cmd.current_dir(board.dir());
    clean_cargo_env(&mut cmd);
    if let Some(p) = &o.port {
        cmd.env("ESPFLASH_PORT", p);
    }
    run(&mut cmd)
}

fn setup(board: Board, o: &Opts) -> Result<(), String> {
    if !full_model().exists() {
        fetch_model()?;
    }
    if !trimmed_model(DEFAULT_DEPTH).exists() {
        trim_model(DEFAULT_DEPTH)?;
    }
    flash_model(board, o)?;
    cargo_firmware(board, "run", o)
}

/// Drops what the outer `cargo run` set for xtask, so the inner cargo picks
/// the firmware's own toolchain and settings.
fn clean_cargo_env(cmd: &mut Command) {
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy();
        if k == "RUSTUP_TOOLCHAIN"
            || k == "CARGO"
            || k == "RUSTC"
            || k == "RUSTDOC"
            || k.starts_with("CARGO_PKG_")
            || k.starts_with("CARGO_MANIFEST_")
            || k == "CARGO_CRATE_NAME"
            || k == "CARGO_BIN_NAME"
            || k == "CARGO_PRIMARY_PACKAGE"
        {
            cmd.env_remove(k.as_ref());
        }
    }
}

fn on_path(exe: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let status = cmd
        .status()
        .map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{:?} failed ({status})", cmd.get_program()))
    }
}
