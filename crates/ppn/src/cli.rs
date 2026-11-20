use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::{env, fs, process::ExitCode};

use clap::{
    ColorChoice, CommandFactory, FromArgMatches, Parser, Subcommand, error::ErrorKind,
    parser::ValueSource,
};
use ppn::config::{self, Env, Flags, Platform, PlatformDirs};
use ppn::error::CliError;
use ppn::flags::{Globals, json_from_env, json_on_command_line};
use ppn::help_tree::example_help;
use ppn::output::{CommandResult, finish};
use ppn::terminal::Context;

use crate::{config_command, debug, help, version};

#[derive(Parser)]
#[command(
    version,
    disable_help_subcommand = true,
    color = ColorChoice::Never,
    about = "An experimental local-first, peer-to-peer task manager",
    after_help = example_help(&["ppn version", "ppn help --json"])
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
    #[command(after_help = example_help(&["ppn version --json"]))]
    Version,
    /// Show help for ppn.
    #[command(after_help = example_help(&["ppn help --json"]))]
    Help {
        /// Command and optional subcommands to describe.
        path: Vec<String>,
    },
    /// Inspect ppn configuration.
    #[command(disable_help_subcommand = true, after_help = example_help(&["ppn config show --json"]))]
    Config {
        #[command(subcommand)]
        command: config_command::Command,
    },
    /// Exercise CLI behavior.
    #[command(
        hide = true,
        disable_help_subcommand = true,
        after_help = example_help(&["ppn debug fail not_found"])
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
            let context = context_for(&Globals::on_early_exit(&args, json_flag));
            return finish(Err(error), json_flag, &context.terminal);
        }
    };
    let parsed = Cli::command()
        .try_get_matches_from(&args)
        .and_then(|mut matches| {
            let explicit_color = matches.value_source("color") == Some(ValueSource::CommandLine);
            Cli::from_arg_matches_mut(&mut matches).map(|cli| (cli, explicit_color))
        });
    match parsed {
        Ok((cli, explicit_color)) => {
            let mut context = context_for(&cli.globals);
            let json = cli.globals.json;
            let result = dispatch(cli, explicit_color, &mut context);
            finish(result, json, &context.terminal)
        }
        Err(error) => {
            let mut definition = Cli::command();
            let json = json_flag || json_env;
            let context = context_for(&Globals::on_early_exit(&args, json));
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

fn dispatch(
    cli: Cli,
    explicit_color: bool,
    context: &mut Context,
) -> Result<CommandResult, CliError> {
    let env: Env = env::vars_os().collect();
    let dirs = platform_dirs(&env)?;
    let flags = Flags {
        config: cli.globals.config,
        color: explicit_color.then_some(cli.globals.color),
    };

    if let Some(Command::Config {
        command: config_command::Command::Path,
    }) = cli.command
    {
        return config_command::path(context, &config::select_files(&flags, &env, &dirs));
    }

    let loaded = config::load(&flags, &env, |path| fs::read_to_string(path), &dirs)?;
    context.config = Some(loaded.config);
    match cli.command {
        Some(Command::Version) => Ok(version::run(context)),
        Some(Command::Help { path }) => {
            let mut definition = Cli::command();
            definition.build();
            help_target(&definition, path.iter().map(String::as_str))
                .map(|command| help::run(context, command.clone(), None))
        }
        Some(Command::Config {
            command: config_command::Command::Show { key },
        }) => config_command::show(context, key.as_deref(), loaded.files),
        Some(Command::Config {
            command: config_command::Command::Path,
        }) => {
            unreachable!("config path is dispatched before loading configuration")
        }
        Some(Command::Debug { command }) => debug::run(context, command),
        None => Ok(help::run(context, Cli::command(), None)),
    }
}

fn platform_dirs(env: &Env) -> Result<PlatformDirs, CliError> {
    let lookup = |name| env.get(OsStr::new(name)).map(Path::new);
    let fallback_home = env::home_dir();
    let home = lookup("HOME")
        .filter(|path| !path.as_os_str().is_empty())
        .or(fallback_home.as_deref());
    let invalid = |message: &str| {
        CliError::new(
            ppn::error::ErrorKind::InvalidConfig,
            format!("HOME: {message}"),
        )
    };
    let home = home.ok_or_else(|| invalid("could not determine the home directory"))?;
    let platform = if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Linux
    };
    PlatformDirs::new(
        platform,
        home,
        lookup("XDG_CONFIG_HOME"),
        lookup("XDG_STATE_HOME"),
    )
    .map_err(|error| invalid(&error.to_string()))
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

    #[test]
    fn context_carries_global_command_options() {
        let args = [
            "ppn",
            "debug",
            "confirm",
            "--timeout",
            "5",
            "--yes",
            "--color=never",
        ];
        let cli = Cli::try_parse_from(args).unwrap();
        let context = context_for(&cli.globals);
        assert_eq!(context.timeout, Some(std::time::Duration::from_secs(5)));
        assert!(context.yes);
        assert!(!context.terminal.color);

        let early = [
            "ppn",
            "version",
            "--help",
            "--timeout=5",
            "--color",
            "never",
            "--yes",
            "--",
            "--timeout=6",
        ]
        .map(OsString::from);
        let context = context_for(&Globals::on_early_exit(&early, false));
        assert_eq!(context.timeout, Some(std::time::Duration::from_secs(5)));
        assert!(context.yes);
        assert!(!context.terminal.color);
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
