// cli.rs - the built binary: exit codes and output of the run kinds.

mod common;
use common::capture_path;
use std::process::{Command, Output};

fn emu(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ipmc-emu"))
        .args(args)
        .output()
        .expect("run ipmc-emu")
}

/// capture -- the capture path as text, None (with the skip line printed) when the file is absent.
fn capture() -> Option<String> {
    if common::capture_missing() {
        return None;
    }
    Some(capture_path().to_string_lossy().into_owned())
}

#[test]
fn the_capture_run_with_both_buses_exits_0() {
    let Some(file) = capture() else { return };
    let out = emu(&[
        "replay",
        &file,
        "--b-fitted",
        "1",
        "--b-enabled",
        "1",
        "--policy",
        "arrival",
    ]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Get Device ID"));
    assert!(text.trim_end().ends_with("PASS"));
}

#[test]
fn the_bench_build_run_exits_0_and_reports_the_unfitted_bus() {
    let Some(file) = capture() else { return };
    let out = emu(&["replay", &file, "--b-fitted", "0"]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("requests on IPMB-B, not fitted in this build: 155"));
    assert!(text.trim_end().ends_with("PASS"));
    let quiet = emu(&["replay", &file, "--b-fitted", "0", "--quiet"]);
    assert_eq!(quiet.status.code(), Some(0));
    let text = String::from_utf8_lossy(&quiet.stdout);
    assert!(text.starts_with("PASS requests=309 answered=154"));
    assert_eq!(text.lines().count(), 1);
}

#[test]
fn a_run_that_loses_responses_exits_1_and_names_the_reason() {
    let Some(file) = capture() else { return };
    let out = emu(&[
        "replay",
        &file,
        "--b-fitted",
        "1",
        "--b-enabled",
        "1",
        "--fail-every",
        "1",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    let last = text.trim_end().lines().last().unwrap_or("");
    assert!(
        last.starts_with("FAIL:") && last.contains("responses lost"),
        "{last}"
    );
}

#[test]
fn trace_prints_the_frames() {
    let Some(file) = capture() else { return };
    let out = emu(&[
        "replay",
        &file,
        "--b-fitted",
        "1",
        "--b-enabled",
        "1",
        "--trace",
        "--quiet",
    ]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.lines().any(|l| l.starts_with("rx IPMB-A")));
    assert!(text.lines().any(|l| l.starts_with("tx IPMB-B ok")));
}

#[test]
fn a_missing_file_exits_2() {
    let out = emu(&["replay", "no/such/file.vec"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!out.stderr.is_empty());
}

#[test]
fn no_arguments_prints_usage_and_exits_2() {
    let out = emu(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Usage:"));
}

#[test]
fn a_bad_option_exits_2() {
    let Some(file) = capture() else { return };
    let bad_policy = emu(&["replay", &file, "--policy", "x"]);
    assert_eq!(bad_policy.status.code(), Some(2));
    // IPMB-B enabled at start but not fitted.
    let not_fitted = emu(&["replay", &file, "--b-enabled", "1", "--b-fitted", "0"]);
    assert_eq!(not_fitted.status.code(), Some(2));
}

#[test]
fn an_empty_vector_file_exits_1_and_names_the_reason() {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("empty.vec");
    std::fs::write(&path, "# no frames\n").expect("write empty vector file");
    let out = emu(&["replay", &path.to_string_lossy()]);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text
        .trim_end()
        .ends_with("FAIL: no requests addressed to the IPMC"));
}

#[test]
fn sdr_prints_one_consistent_line_per_record() {
    let out = emu(&["sdr"]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().collect();
    // 7 sensor records and the locator; the Payload Mode sensor adds one at Tier-3.
    assert_eq!(lines.len(), if ipmc_defs::IPMC_TIER == 3 { 9 } else { 8 });
    for (i, line) in lines.iter().enumerate() {
        let (id, hex) = line.split_once(' ').expect("id and bytes");
        assert_eq!(id, format!("{i:04X}"));
        let bytes: Vec<u8> = hex
            .split(' ')
            .map(|b| u8::from_str_radix(b, 16).expect("hex byte"))
            .collect();
        // Record header: ID (2), version, type, length; the length counts the bytes after it.
        assert_eq!(usize::from(bytes[4]) + 5, bytes.len());
    }
}

#[test]
fn serve_rejects_bad_options_with_exit_2() {
    let cases: [&[&str]; 4] = [
        &["serve", "--bogus"],
        &["serve", "--port", "http"],
        &["serve", "--fail-every", "3"],
        &["serve", "some.vec"],
    ];
    for args in cases {
        let out = emu(args);
        assert_eq!(out.status.code(), Some(2), "args {args:?}");
        assert!(!out.stderr.is_empty(), "args {args:?}");
    }
}
