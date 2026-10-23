// build.rs - parses ipmi_defs.h into $OUT_DIR/ipmi_defs.rs (`pub const NAME: u32` per integer
// define, `pub const NAME: &str` per string define).
//
// The parser and its rules live in defs_parser.rs (shared with the unit tests); a rejected
// header fails the build with `path:line: reason`.
// Environment: IPMI_DEFS_H overrides the header path (used to prove that a bad line fails the build).

use std::env;
use std::fs;
use std::path::PathBuf;

#[path = "defs_parser.rs"]
mod defs_parser;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    println!("cargo:rerun-if-env-changed=IPMI_DEFS_H");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=defs_parser.rs");
    let header = match env::var("IPMI_DEFS_H") {
        Ok(p) => PathBuf::from(p),
        Err(_) => manifest.join("../../ipmi_defs.h"),
    };
    println!("cargo:rerun-if-changed={}", header.display());
    let path = header.display().to_string();
    // Lossy decoding turns an invalid UTF-8 byte into U+FFFD, which the parser rejects with its
    // line number if it sits in a string literal.
    let bytes = fs::read(&header).unwrap_or_else(|e| panic!("{path}: {e}"));
    let raw = String::from_utf8_lossy(&bytes);
    let defines =
        defs_parser::parse(&raw).unwrap_or_else(|e| panic!("{path}:{}: {}", e.line, e.why));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("ipmi_defs.rs"), defs_parser::render(&defines))
        .expect("write ipmi_defs.rs");
}
