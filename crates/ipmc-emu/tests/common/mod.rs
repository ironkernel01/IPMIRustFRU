// common/mod.rs - the path of the capture and a loader call for the integration tests. The
// reader itself is ipmc_emu::vectors, the only one.

use ipmc_emu::vectors::{self, Vector};
use std::path::PathBuf;

// Each integration test binary uses a different subset.
#[allow(dead_code)]
/// capture_path -- vectors/sbc_ab.vec in the repository.
pub fn capture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vectors/sbc_ab.vec")
}

// Each integration test binary uses a different subset.
#[allow(dead_code)]
/// load -- the frames of the capture; panics with the reader's message on failure (test code).
pub fn load() -> Vec<Vector> {
    vectors::load(&capture_path()).unwrap_or_else(|e| panic!("load capture: {e}"))
}

// Each integration test binary uses a different subset.
#[allow(dead_code)]
/// capture_missing -- true when the capture file is absent; prints one line naming the file and
/// the reason, and the calling test then returns early (passes). The capture is local only and
/// git-ignored (owner directive 10-23-2026), so a fresh clone does not have it. Every test that
/// reads the capture starts with `if common::capture_missing() { return; }`.
pub fn capture_missing() -> bool {
    let path = capture_path();
    if path.exists() {
        return false;
    }
    println!(
        "SKIPPED: {} is absent (the capture is local only and git-ignored)",
        path.display()
    );
    true
}
