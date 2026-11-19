use clap::{Command, builder::StyledStr};
use ppn::error::ErrorKind;
use ppn::help_tree::CommandInfo;
use ppn::output::{CommandResult, Render};
use ppn::terminal::Terminal;
use serde::Serialize;

#[derive(Serialize)]
struct Help {
    #[serde(flatten)]
    command: CommandInfo,
    exit_codes: [ExitStatus; 7],
    #[serde(skip)]
    text: StyledStr,
}

#[derive(Serialize)]
struct ExitStatus {
    status: u8,
    code: &'static str,
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

pub fn run(mut command: Command, help_text: Option<StyledStr>) -> CommandResult {
    command.build();
    let text = help_text.unwrap_or_else(|| command.render_help());
    let exit_codes = [
        None,
        Some(ErrorKind::Failure),
        Some(ErrorKind::Usage),
        Some(ErrorKind::NotFound),
        Some(ErrorKind::AmbiguousId),
        Some(ErrorKind::PermissionDenied),
        Some(ErrorKind::DaemonUnreachable),
    ]
    .map(|kind| match kind {
        None => ExitStatus {
            status: 0,
            code: "ok",
        },
        Some(kind) => ExitStatus {
            status: kind.status(),
            code: kind.code(),
        },
    });
    CommandResult::document(Help {
        command: CommandInfo::from_command(&command),
        exit_codes,
        text,
    })
}
