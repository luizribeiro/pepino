#![deny(clippy::print_stdout)]

use std::{io, process::ExitCode};

use clap::{ColorChoice, Parser, error::ErrorKind};
use ppn::error::CliError;

#[derive(Parser)]
#[command(
    version,
    color = ColorChoice::Never,
    about = "An experimental local-first, peer-to-peer task manager"
)]
struct Cli {}

fn main() -> ExitCode {
    match Cli::try_parse() {
        Ok(_) => ExitCode::SUCCESS,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            let _ = error.print();
            ExitCode::SUCCESS
        }
        Err(error) => CliError::usage(error).report(false, io::stderr().lock()),
    }
}
