//! TingBuDont binary entry point. The pipeline lives in the library crate so
//! tests and examples can drive it directly; this `main` just parses args and
//! dispatches.

use std::process::ExitCode;

fn main() -> ExitCode {
    match tingbudont::cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(tingbudont::cli::CliError::Usage(msg)) => {
            eprintln!("tingbudont: {msg}");
            ExitCode::from(2)
        }
        Err(tingbudont::cli::CliError::Failure(msg)) => {
            eprintln!("tingbudont: {msg}");
            ExitCode::from(1)
        }
    }
}
