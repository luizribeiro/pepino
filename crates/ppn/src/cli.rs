use std::{env, process::ExitCode};

use clap::{ColorChoice, CommandFactory, Parser, Subcommand, error::ErrorKind};
use ppn::error::CliError;
use ppn::flags::{Globals, color_on_early_exit};
use ppn::output::finish;
use ppn::terminal::Terminal;

use crate::{debug, help, version};

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
    /// Exercise CLI behavior.
    #[command(
        hide = true,
        disable_help_subcommand = true,
        after_help = "Examples:\n  ppn debug fail not_found"
    )]
    Debug {
        #[command(subcommand)]
        command: debug::Command,
    },
}

pub fn run() -> ExitCode {
    let args = env::args_os().collect::<Vec<_>>();
    match Cli::try_parse_from(&args) {
        Ok(cli) => {
            let terminal = Terminal::detect(cli.globals.color, cli.globals.yes);
            let result = match cli.command {
                Some(Command::Version) => Ok(version::run()),
                Some(Command::Help) => Ok(help::run(Cli::command(), None)),
                Some(Command::Debug { command }) => debug::run(command, &terminal),
                None => return ExitCode::SUCCESS,
            };
            finish(result, cli.globals.json, &terminal)
        }
        Err(error) => {
            let definition = Cli::command();
            let json = Globals::json_on_early_exit(&args, &definition);
            let result = match error.kind() {
                ErrorKind::DisplayHelp => Ok(help::run(definition, Some(error.render()))),
                ErrorKind::DisplayVersion => Ok(version::run()),
                _ => Err(CliError::usage(error)),
            };
            finish(
                result,
                json,
                &Terminal::detect(color_on_early_exit(&args), false),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ppn::help_tree::examples;

    #[test]
    fn every_command_has_a_one_line_summary_and_an_example() {
        let mut command = Cli::command();
        command.build();
        assert_documented(&command);
    }

    fn assert_documented(command: &clap::Command) {
        let name = command.get_bin_name().unwrap_or_else(|| command.get_name());
        let summary = command
            .get_about()
            .map(ToString::to_string)
            .unwrap_or_default();
        assert_eq!(
            summary.trim().lines().count(),
            1,
            "{name} needs a one-line summary"
        );

        let rendered = command.clone().render_long_help().to_string();
        assert!(
            examples(command)
                .iter()
                .any(|example| rendered.lines().any(|line| line.trim() == example)),
            "{name} needs an example in its help text"
        );

        for child in command.get_subcommands() {
            assert_documented(child);
        }
    }
}
