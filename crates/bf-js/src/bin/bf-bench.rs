//! `bf-bench`: el benchmark de bf-js (lo mismo que `bf-js bench`; ver `bench/mod.rs`).

use std::process::ExitCode;

mod bench;

fn main() -> ExitCode {
    bench::main(std::env::args().skip(1).collect())
}
