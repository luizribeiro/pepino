use std::{env, process::ExitCode};

use clap::{ColorChoice, CommandFactory, Parser, Subcommand, error::ErrorKind};
use ppn::error::CliError;
use ppn::flags::Globals;
use ppn::output::finish;
use ppn::terminal::{Color, Terminal};

use crate::version;

#[derive(Parser)]
#[command(
    version,
    color = ColorChoice::Never,
    about = "An experimental local-first, peer-to-peer task manager",
    after_help = "Examples:\n  ppn version\n  ppn --help"
)]
struct Cli {
    #[command(flatten)]
    globals: Globals,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Print the installed ppn version.
    #[command(after_help = "Examples:\n  ppn version --json")]
    Version,
}

pub fn run() -> ExitCode {
    let args = env::args_os().collect::<Vec<_>>();
    match Cli::try_parse_from(&args) {
        Ok(cli) => {
            let terminal = Terminal::detect(cli.globals.color, cli.globals.yes);
            let result = match cli.command {
                Some(Command::Version) => version::run(),
                None => return ExitCode::SUCCESS,
            };
            finish(Ok(result), cli.globals.json, &terminal)
        }
        Err(error) if error.kind() == ErrorKind::DisplayHelp => {
            let _ = error.print();
            ExitCode::SUCCESS
        }
        Err(error) => {
            let json = Globals::json_on_parse_error(&args, &Cli::command());
            let result = match error.kind() {
                ErrorKind::DisplayVersion => Ok(version::run()),
                _ => Err(CliError::usage(error)),
            };
            finish(result, json, &Terminal::detect(Color::Auto, false))
        }
    }
}
