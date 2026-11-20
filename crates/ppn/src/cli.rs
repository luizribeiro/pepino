use std::ffi::OsString;
use std::{env, process::ExitCode};

use clap::{ColorChoice, CommandFactory, Parser, Subcommand, error::ErrorKind};
use ppn::error::CliError;
use ppn::flags::{Globals, color_on_early_exit, json_from_env, json_on_command_line};
use ppn::output::finish;
use ppn::terminal::Context;

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
    Help {
        /// Command and optional subcommands to describe.
        path: Vec<String>,
    },
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
    let json_flag = json_on_command_line(&args);
    let json_env = match json_from_env() {
        Ok(json) => json,
        Err(error) => {
            return finish(
                Err(error),
                json_flag,
                &Context::detect(color_on_early_exit(&args), false, None).terminal,
            );
        }
    };
    match Cli::try_parse_from(&args) {
        Ok(cli) => {
            let context = context_for(&cli.globals);
            let result = match cli.command {
                Some(Command::Version) => Ok(version::run(&context)),
                Some(Command::Help { path }) => {
                    let mut definition = Cli::command();
                    definition.build();
                    help_target(&definition, path.iter().map(String::as_str))
                        .map(|command| help::run(&context, command.clone(), None))
                }
                Some(Command::Debug { command }) => debug::run(&context, command),
                None => Ok(help::run(&context, Cli::command(), None)),
            };
            finish(result, cli.globals.json, &context.terminal)
        }
        Err(error) => {
            let mut definition = Cli::command();
            let json = json_flag || json_env;
            let context = Context::detect(color_on_early_exit(&args), false, None);
            let result = match error.kind() {
                ErrorKind::DisplayHelp => {
                    definition.build();
                    help_flag_target(&definition, &args, json)
                        .map(|command| help::run(&context, command.clone(), Some(error.render())))
                }
                ErrorKind::DisplayVersion => Ok(version::run(&context)),
                _ => Err(CliError::usage(error)),
            };
            finish(result, json, &context.terminal)
        }
    }
}

fn context_for(globals: &Globals) -> Context {
    Context::detect(globals.color, globals.yes, globals.timeout)
}

fn help_flag_target<'a>(
    definition: &'a clap::Command,
    args: &[OsString],
    json: bool,
) -> Result<&'a clap::Command, CliError> {
    if !json {
        return Ok(definition);
    }
    // clap returns the rendered help without the command that produced it.
    let mut probe = mark_help_paths(definition.clone(), "");
    match probe.try_get_matches_from_mut(args) {
        Err(error) if error.kind() == ErrorKind::DisplayHelp => {
            help_target(definition, error.render().to_string().split_whitespace())
        }
        _ => Err(CliError::new(
            ppn::error::ErrorKind::Failure,
            "Could not resolve help target.",
        )),
    }
}

fn mark_help_paths(command: clap::Command, path: &str) -> clap::Command {
    command
        .override_help(path.to_owned())
        .mut_subcommands(|child| {
            let path = format!("{path} {}", child.get_name());
            mark_help_paths(child, &path)
        })
}

fn help_target<'a, 'b>(
    mut command: &'a clap::Command,
    path: impl IntoIterator<Item = &'b str>,
) -> Result<&'a clap::Command, CliError> {
    for name in path {
        command = command.find_subcommand(name).ok_or_else(|| {
            CliError::usage(command.clone().error(
                ErrorKind::InvalidSubcommand,
                format!("unrecognized subcommand '{name}'"),
            ))
        })?;
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ppn::help_tree::{argument_summary, examples};

    #[test]
    fn every_command_and_argument_is_documented() {
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

        for arg in command.get_arguments() {
            assert!(
                argument_summary(arg).is_some_and(|summary| !summary.trim().is_empty()),
                "{name} argument {} needs help text",
                arg.get_id()
            );
        }

        for child in command.get_subcommands() {
            assert_documented(child);
        }
    }
}
#[test]
fn context_carries_global_command_options() {
    let cli = Cli::try_parse_from([
        "ppn",
        "debug",
        "confirm",
        "--timeout",
        "5",
        "--yes",
        "--color",
        "never",
    ])
    .unwrap();
    let context = context_for(&cli.globals);
    assert_eq!(context.timeout, Some(std::time::Duration::from_secs(5)));
    assert!(context.yes);
    assert!(!context.terminal.color);
}
