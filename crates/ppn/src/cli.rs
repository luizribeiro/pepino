use std::{env, process::ExitCode};

use clap::{ColorChoice, CommandFactory, Parser, Subcommand, error::ErrorKind};
use ppn::error::CliError;
use ppn::flags::Globals;
use ppn::output::finish;
use ppn::terminal::{Color, Terminal};

use crate::{help, version};

#[derive(Parser)]
#[command(
    version,
    disable_help_subcommand = true,
    color = ColorChoice::Never,
    about = "An experimental local-first, peer-to-peer task manager",
    after_help = "Examples:\n  ppn version\n  ppn help --json"
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
    /// Show help for ppn.
    #[command(after_help = "Examples:\n  ppn help --json")]
    Help,
}

pub fn run() -> ExitCode {
    let args = env::args_os().collect::<Vec<_>>();
    match Cli::try_parse_from(&args) {
        Ok(cli) => {
            let terminal = Terminal::detect(cli.globals.color, cli.globals.yes);
            let result = match cli.command {
                Some(Command::Version) => version::run(),
                Some(Command::Help) | None => help::run(Cli::command(), None),
            };
            finish(Ok(result), cli.globals.json, &terminal)
        }
        Err(error) => {
            let json = Globals::json_on_parse_error(&args, &Cli::command());
            let result = match error.kind() {
                ErrorKind::DisplayHelp => Ok(help::run(Cli::command(), Some(error.render()))),
                ErrorKind::DisplayVersion => Ok(version::run()),
                _ => Err(CliError::usage(error)),
            };
            finish(result, json, &Terminal::detect(Color::Auto, false))
        }
    }
}
