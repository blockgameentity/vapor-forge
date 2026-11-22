//! Behavioural candidate generator for Windows Steam modules; see
//! `pe_probe` for what it measures and why the output is evidence rather than
//! a verdict.
//!
//! `--dll` and `--module` are required, `--toml` names the mined corpus of that
//! build so direct callees can be named, `--against` compares candidates with a
//! second build, `--dump <rva>` prints one function for a human to read.

mod pe_probe;

fn main() {
    if let Err(error) = pe_probe::run() {
        eprintln!("vapor-forge-pe-probe: {error}");
        std::process::exit(1);
    }
}
