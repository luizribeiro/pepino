use clap::{Command, builder::StyledStr};
use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::command_fields;
use ppn::output::{CommandResult, Render};
use ppn::terminal::{Context, Terminal};
use serde::Serialize;
use serde_json::{Map, Value, json};

#[path = "help_topics.rs"]
mod topics;

struct Help {
    command: Map<String, Value>,
    exit_codes: Vec<ExitStatus>,
    error_codes: Vec<ErrorCode>,
    text: StyledStr,
}

#[derive(Serialize)]
struct ExitStatus {
    status: u8,
    code: &'static str,
    error_codes: Vec<&'static str>,
}

#[derive(Serialize)]
struct ErrorCode {
    code: &'static str,
    status: u8,
    meaning: &'static str,
}

fn exit_codes() -> Vec<ExitStatus> {
    let mut codes = std::collections::BTreeMap::new();
    for kind in ErrorKind::all() {
        codes
            .entry(kind.status())
            .or_insert_with(Vec::new)
            .push(kind.code());
    }
    std::iter::once(ExitStatus {
        status: 0,
        code: "ok",
        error_codes: Vec::new(),
    })
    .chain(codes.into_iter().map(|(status, error_codes)| ExitStatus {
        status,
        code: error_codes[0],
        error_codes,
    }))
    .collect()
}

impl Render for Help {
    fn human(&self, terminal: &Terminal) -> String {
        if terminal.color {
            self.text.ansi().to_string()
        } else {
            self.text.to_string()
        }
    }

    fn json_fields(mut self) -> Map<String, Value> {
        self.command
            .insert("exit_codes".into(), json!(self.exit_codes));
        self.command
            .insert("error_codes".into(), json!(self.error_codes));
        self.command.insert("topics".into(), json!(topics::all()));
        self.command
    }
}

pub fn run(_: &Context, mut command: Command, help_text: Option<StyledStr>) -> CommandResult {
    command.build();
    let text = help_text.unwrap_or_else(|| {
        match command.try_get_matches_from_mut([command.get_name().to_owned(), "--help".into()]) {
            Err(error) if error.kind() == clap::error::ErrorKind::DisplayHelp => error.render(),
            _ => command.render_help(),
        }
    });
    CommandResult::document(Help {
        command: command_fields(&command),
        exit_codes: exit_codes(),
        error_codes: ErrorKind::all()
            .iter()
            .map(|kind| ErrorCode {
                code: kind.code(),
                status: kind.status(),
                meaning: kind.meaning(),
            })
            .collect(),
        text,
    })
}

#[derive(clap::Args)]
#[command(about = "Show help for ppn.", after_help = ppn::help_tree::example_help(&["ppn help --json"]))]
pub struct Args {
    /// Command, subcommand or topic (filters, dates, ids, json) to describe.
    path: Vec<String>,
}

pub fn for_path(
    context: &Context<'_>,
    mut definition: Command,
    args: Args,
) -> Result<CommandResult, CliError> {
    definition.build();
    if let [name] = args.path.as_slice()
        && let Some(text) = topics::all().remove(name.as_str())
    {
        return Ok(CommandResult::document(topics::Topic(name.clone(), text)));
    }
    let command = help_target(&definition, args.path.iter().map(String::as_str))?;
    Ok(run(context, command.clone(), None))
}

pub(crate) fn help_target<'a, 'b>(
    mut command: &'a clap::Command,
    path: impl IntoIterator<Item = &'b str>,
) -> Result<&'a clap::Command, CliError> {
    for name in path {
        command = command.find_subcommand(name).ok_or_else(|| {
            CliError::usage(command.clone().error(
                clap::error::ErrorKind::InvalidSubcommand,
                format!("unrecognized subcommand '{name}'"),
            ))
        })?;
    }
    Ok(command)
}
