//! Console report over a directory of CUE4Parse JSON.
//!
//! Reads the directory and writes the report to stdout. Creates no files, caches, or temp output.
//! Under a CUE4Parse `Content` root it also prints the ticket 2 sections (`format_read_report`).
//!
//!   cargo run --manifest-path rust/Cargo.toml -p gore-core --bin horde_report -- <cue4parse-json-dir>

use std::env;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(dir) = args.next() else {
        eprintln!("usage: horde_report <cue4parse-json-dir>");
        return ExitCode::from(2);
    };
    if args.next().is_some() {
        eprintln!("usage: horde_report <cue4parse-json-dir>");
        return ExitCode::from(2);
    }
    match gore_core::horde::scan_dir(Path::new(&dir)) {
        Ok(scan) => {
            print!("{}", gore_core::horde::format_report(&scan.layout, &scan.files_scanned, &scan.files_skipped));
            print!("{}", gore_core::horde::format_read_report(&scan));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}
