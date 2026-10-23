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
