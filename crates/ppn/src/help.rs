use clap::{Command, builder::StyledStr};
use ppn::error::ErrorKind;
use ppn::help_tree::CommandInfo;
use ppn::output::{CommandResult, Render};
use ppn::terminal::{Context, Terminal};
use serde::Serialize;

#[derive(Serialize)]
struct Help {
    #[serde(flatten)]
    command: CommandInfo,
    exit_codes: Vec<ExitStatus>,
    error_codes: Vec<ErrorCode>,
    #[serde(skip)]
    text: StyledStr,
}

#[derive(Serialize)]
struct ExitStatus {
    status: u8,
    code: &'static str,
}

#[derive(Serialize)]
struct ErrorCode {
    code: &'static str,
    status: u8,
    meaning: &'static str,
}

fn exit_codes() -> Vec<ExitStatus> {
    let mut codes = vec![ExitStatus {
        status: 0,
        code: "ok",
    }];
    codes.extend(ErrorKind::all().iter().map(|kind| ExitStatus {
        status: kind.status(),
        code: kind.code(),
    }));
    codes.sort_by_key(|entry| entry.status);
    codes.dedup_by_key(|entry| entry.status);
    codes
}

impl Render for Help {
    fn human(&self, terminal: &Terminal) -> String {
        if terminal.color {
            self.text.ansi().to_string()
        } else {
            self.text.to_string()
        }
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
        command: CommandInfo::from_command(&command),
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
