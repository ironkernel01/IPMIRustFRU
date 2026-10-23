//! main.rs - command line of the emulator: argument parsing, help text and exit codes only.
//! The behaviour is in the library (see its crate documentation).
#![forbid(unsafe_code)]

use ipmc_core::link::TxPolicy;
use ipmc_core::trace::{NoTrace, Trace};
use ipmc_emu::emulator::{EmuConfig, Emulator};
use ipmc_emu::replay::{replay, ReplayOptions};
use ipmc_emu::trace::StdoutTrace;
use ipmc_emu::vectors;
use std::path::PathBuf;
use std::process::ExitCode;

/// NO_NEXT_RECORD -- the next-ID value after the last record (IPMI v2.0 section 33.12).
const NO_NEXT_RECORD: u16 = 0xFFFF;

/// EXIT_FAIL -- the replay ran and did not pass.
const EXIT_FAIL: u8 = 1;
/// EXIT_USAGE -- bad arguments, unreadable file or bad vector line.
const EXIT_USAGE: u8 = 2;

const USAGE: &str = "\
ipmc-emu - the IPMC on the PC

Usage:
  ipmc-emu replay <file.vec> [options]
  ipmc-emu sdr
  ipmc-emu help

Options for replay (defaults from ipmi_defs.h):
  --b-fitted 0|1      IPMB-B is built in
  --b-enabled 0|1     IPMB-B starts enabled (needs --b-fitted 1; default 0)
  --policy alternate|arrival
                      response bus policy
  --fail-every N      report every Nth transmission failed (default 0, none)
  --trace             print every frame and trace line
  --quiet             print a one-line summary only

sdr prints the Device SDR repository, one line per record:
  <record ID, 4 hex digits> <record bytes, hex, space separated>

Exit status: 0 passed, 1 not passed (replay) or a broken record chain (sdr), 2 usage or file error.";

/// Args -- the parsed command line of `replay`.
struct Args {
    file: PathBuf,
    config: EmuConfig,
    options: ReplayOptions,
    trace: bool,
    quiet: bool,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => {
            println!("{USAGE}");
            if args.is_empty() {
                return ExitCode::from(EXIT_USAGE);
            }
            ExitCode::SUCCESS
        }
        Some("replay") => match parse_replay(&args[1..]) {
            Ok(parsed) => run_replay(parsed),
            Err(message) => usage_error(&message),
        },
        Some("sdr") if args.len() == 1 => run_sdr(),
        Some("sdr") => usage_error("sdr takes no arguments"),
        Some(other) => usage_error(&format!("unknown command '{other}'")),
    }
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("error: {message}");
    eprintln!("{USAGE}");
    ExitCode::from(EXIT_USAGE)
}

fn parse_replay(args: &[String]) -> Result<Args, String> {
    let mut file: Option<PathBuf> = None;
    let mut config = EmuConfig::from_defs();
    let mut options = ReplayOptions::default();
    let (mut trace, mut quiet) = (false, false);
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = |name: &str| -> Result<&String, String> {
            iter.next().ok_or(format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--b-fitted" => config.b_fitted = flag(value("--b-fitted")?, "--b-fitted")?,
            "--b-enabled" => {
                config.b_enabled_at_start = flag(value("--b-enabled")?, "--b-enabled")?;
            }
            "--policy" => {
                config.tx_policy = match value("--policy")?.as_str() {
                    "alternate" => TxPolicy::Alternate,
                    "arrival" => TxPolicy::Arrival,
                    other => {
                        return Err(format!("--policy: '{other}' is not alternate or arrival"))
                    }
                };
            }
            "--fail-every" => {
                let text = value("--fail-every")?;
                options.fail_every = text
                    .parse()
                    .map_err(|_| format!("--fail-every: '{text}' is not a number"))?;
            }
            "--trace" => trace = true,
            "--quiet" => quiet = true,
            other if other.starts_with("--") => return Err(format!("unknown option '{other}'")),
            path => {
                if file.replace(PathBuf::from(path)).is_some() {
                    return Err("more than one vector file given".to_string());
                }
            }
        }
    }
    let file = file.ok_or("replay needs a vector file".to_string())?;
    Ok(Args {
        file,
        config,
        options,
        trace,
        quiet,
    })
}

/// flag -- "0" or "1" to a bool.
fn flag(text: &str, name: &str) -> Result<bool, String> {
    match text {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(format!("{name}: '{text}' is not 0 or 1")),
    }
}

fn run_replay(args: Args) -> ExitCode {
    let vectors = match vectors::load(&args.file) {
        Ok(v) => v,
        Err(e) => return usage_error(&format!("{}: {e}", args.file.display())),
    };
    let mut emu = match Emulator::new(args.config) {
        Ok(emu) => emu,
        Err(e) => return usage_error(&e.to_string()),
    };
    let mut stdout_trace = StdoutTrace;
    let mut no_trace = NoTrace;
    let trace: &mut dyn Trace = if args.trace {
        &mut stdout_trace
    } else {
        &mut no_trace
    };
    let report = replay(&mut emu, &vectors, &args.options, trace);
    if args.quiet {
        println!("{}", report.summary_line());
    } else {
        println!("{report}");
    }
    if report.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_FAIL)
    }
}

/// run_sdr -- prints every record of the Device SDR repository, from record 0000h along the
/// next-ID chain to FFFFh, at most one step more than the record count. A chain that does not
/// end in that many steps or names a missing record exits 1.
fn run_sdr() -> ExitCode {
    let emu = match Emulator::new(EmuConfig::from_defs()) {
        Ok(emu) => emu,
        Err(e) => return usage_error(&e.to_string()),
    };
    let sdr = &emu.context().sdr;
    let mut id = 0u16;
    for _ in 0..=sdr.record_count() {
        let Some(bytes) = sdr.record(id) else {
            eprintln!("error: SDR record {id:04X} does not exist");
            return ExitCode::from(EXIT_FAIL);
        };
        let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02X}")).collect();
        println!("{id:04X} {}", hex.join(" "));
        id = sdr.next_id(id);
        if id == NO_NEXT_RECORD {
            return ExitCode::SUCCESS;
        }
    }
    eprintln!("error: SDR record chain does not end");
    ExitCode::from(EXIT_FAIL)
}
