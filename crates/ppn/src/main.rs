#![deny(clippy::print_stdout)]

use std::{env, io, process::ExitCode};

use clap::{ColorChoice, CommandFactory, Parser, error::ErrorKind};
use ppn::{error::CliError, flags::Globals};

#[derive(Parser)]
#[command(
    version,
    color = ColorChoice::Never,
    about = "An experimental local-first, peer-to-peer task manager"
)]
struct Cli {
    #[command(flatten)]
    globals: Globals,
}

fn main() -> ExitCode {
    let args = env::args_os().collect::<Vec<_>>();
    match Cli::try_parse_from(&args) {
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
        Err(error) => CliError::usage(error).report(
            Globals::json_on_parse_error(&args, &Cli::command()),
            io::stderr().lock(),
        ),
    }
}
