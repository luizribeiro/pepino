use std::cell::OnceCell;
use std::ffi::OsStr;
use std::io::{self, BufRead, IsTerminal, Write};
use std::time::Duration;

use clap::{ValueEnum, builder::styling::AnsiColor};
use serde::Serialize;

use crate::app::InvocationTime;
use crate::config::{Env, LoadedConfig};
use crate::error::{CliError, ErrorKind};
use crate::render::Dates;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Clone, Copy, Debug)]
pub struct Terminal {
    pub stdout_is_tty: bool,
    pub stdin_is_tty: bool,
    pub color: bool,
    pub width: Option<usize>,
    no_color: bool,
    dumb: bool,
}

impl Terminal {
    pub fn detect(color: Color, env: &Env) -> Self {
        let stdout = io::stdout();
        let stdout_is_tty = stdout.is_terminal();
        let width = terminal_size::terminal_size_of(&stdout)
            .map(|(width, _)| usize::from(width.0))
            .filter(|width| stdout_is_tty && *width > 0);
        let stdin_is_tty = io::stdin().is_terminal();
        let no_color = env
            .get(OsStr::new("NO_COLOR"))
            .is_some_and(|value| !value.is_empty());
        let dumb = env
            .get(OsStr::new("TERM"))
            .is_some_and(|value| value == "dumb");
        Self {
            stdout_is_tty,
            stdin_is_tty,
            color: use_color(color, stdout_is_tty, no_color, dumb),
            width,
            no_color,
            dumb,
        }
    }

    pub fn paint(&self, text: &str, color: AnsiColor) -> String {
        if !self.color {
            return text.to_owned();
        }
        let style = color.on_default();
        format!("{}{text}{}", style.render(), style.render_reset())
    }

    pub fn task_output(self, color: Color) -> Self {
        Self {
            color: use_color(color, self.stdout_is_tty, self.no_color, self.dumb),
            ..self
        }
    }

    #[cfg(test)]
    pub(crate) fn fixed(width: Option<usize>, color: bool) -> Self {
        Self {
            stdout_is_tty: width.is_some(),
            stdin_is_tty: false,
            color,
            width,
            no_color: false,
            dumb: false,
        }
    }
}

pub struct RenderSettings {
    pub terminal: Terminal,
    time: InvocationTime,
    date_format: String,
}

impl RenderSettings {
    pub fn dates(&self) -> Dates<'_> {
        Dates {
            time: self.time,
            format: &self.date_format,
        }
    }
}

pub struct Context<'a> {
    pub terminal: Terminal,
    pub(crate) wrote: std::rc::Rc<std::cell::Cell<bool>>,
    pub(crate) board_notice: std::rc::Rc<std::cell::RefCell<Option<crate::output::Warning>>>,
    config: OnceCell<Result<LoadedConfig, CliError>>,
    load_config: Box<dyn Fn() -> Result<LoadedConfig, CliError> + 'a>,
    pub yes: bool,
    pub timeout: Option<Duration>,
    pub time: InvocationTime,
    pub board_flag: Option<String>,
    pub board_env: Option<std::ffi::OsString>,
}

impl<'a> Context<'a> {
    pub fn new(
        terminal: Terminal,
        yes: bool,
        timeout: Option<Duration>,
        time: InvocationTime,
        load_config: impl Fn() -> Result<LoadedConfig, CliError> + 'a,
    ) -> Self {
        Self {
            terminal,
            config: OnceCell::new(),
            wrote: Default::default(),
            board_notice: Default::default(),
            load_config: Box::new(load_config),
            yes,
            timeout,
            time,
            board_flag: None,
            board_env: None,
        }
    }

    pub fn board_override(&self) -> Result<Option<(&str, &'static str)>, CliError> {
        if let Some(board) = &self.board_flag {
            return Ok(Some((board, "--board")));
        }
        self.board_env
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(|value| {
                value
                    .to_str()
                    .map(|board| (board, "PPN_BOARD"))
                    .ok_or_else(|| {
                        CliError::new(
                            ErrorKind::InvalidInput,
                            "PPN_BOARD must contain a UTF-8 board reference.",
                        )
                    })
            })
            .transpose()
    }

    pub fn take_board_notice(&self) -> Option<crate::output::Warning> {
        self.board_notice.take()
    }

    pub fn wrote(&self) -> bool {
        self.wrote.get()
    }

    pub fn config(&self) -> Result<&LoadedConfig, CliError> {
        self.config
            .get_or_init(&self.load_config)
            .as_ref()
            .map_err(Clone::clone)
    }

    pub fn render_settings(&self) -> Result<RenderSettings, CliError> {
        let ui = &self.config()?.config.ui;
        Ok(RenderSettings {
            terminal: self.terminal.task_output(ui.color.value),
            time: self.time,
            date_format: ui.date_format.value.as_str().to_owned(),
        })
    }

    pub fn confirm(&self, prompt: &str) -> Result<bool, CliError> {
        self.confirm_with_refusal(prompt, prompt)
    }

    pub fn confirm_with_refusal(&self, prompt: &str, refusal: &str) -> Result<bool, CliError> {
        if self.yes {
            return Ok(true);
        }
        if !self.terminal.stdin_is_tty {
            return Err(CliError::new(ErrorKind::ConfirmationRequired, refusal)
                .with_hint("Pass --yes to confirm."));
        }

        ask(prompt, io::stdin().lock(), io::stderr().lock()).map_err(|error| {
            CliError::new(
                ErrorKind::Failure,
                format!("Could not read confirmation: {error}"),
            )
        })
    }
}

fn use_color(color: Color, stdout_is_tty: bool, no_color: bool, dumb: bool) -> bool {
    match color {
        Color::Auto => stdout_is_tty && !no_color && !dumb,
        Color::Always => true,
        Color::Never => false,
    }
}

fn ask(prompt: &str, mut stdin: impl BufRead, mut stderr: impl Write) -> io::Result<bool> {
    write!(stderr, "{prompt} [y/N] ")?;
    stderr.flush()?;
    let mut answer = String::new();
    stdin.read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_the_first_configuration_error() {
        let attempts = std::cell::Cell::new(0);
        let context = Context::new(
            Terminal::detect(Color::Never, &Env::new()),
            false,
            None,
            InvocationTime {
                now_ms: 0,
                offset: chrono::FixedOffset::east_opt(0).unwrap(),
                today: "2026-11-20".parse().unwrap(),
            },
            || {
                attempts.set(attempts.get() + 1);
                Err(CliError::new(
                    ErrorKind::InvalidConfig,
                    "PPN_MDNS: expected a boolean",
                ))
            },
        );
        assert_eq!(attempts.get(), 0);
        for _ in 0..2 {
            assert_eq!(
                context.config().unwrap_err().kind(),
                ErrorKind::InvalidConfig
            );
        }
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn only_automatic_color_depends_on_the_terminal_and_environment() {
        assert!(use_color(Color::Always, false, false, false));
        assert!(use_color(Color::Always, false, true, true));
        assert!(!use_color(Color::Auto, false, false, false));
        assert!(!use_color(Color::Auto, true, true, false));
        assert!(!use_color(Color::Auto, true, false, true));
        assert!(use_color(Color::Auto, true, false, false));
        assert!(use_color(Color::Always, true, false, true));
        assert!(!use_color(Color::Never, true, false, false));
    }

    #[test]
    fn task_color_honors_explicit_modes_and_limits_auto_to_terminals() {
        let tty = Terminal::fixed(Some(48), false);
        assert_eq!(
            tty.task_output(Color::Always)
                .paint("Ready", AnsiColor::Green),
            "[32mReady[0m"
        );
        let muted = Terminal {
            no_color: true,
            ..tty
        };
        assert!(muted.task_output(Color::Always).color);
        assert!(!muted.task_output(Color::Auto).color);
        let pipe = Terminal::fixed(None, false);
        assert!(pipe.task_output(Color::Always).color);
        assert!(!pipe.task_output(Color::Auto).color);
    }

    #[test]
    fn asks_on_stderr_and_accepts_explicit_yes() {
        let mut stderr = Vec::new();
        let prompt = "Archive the release board?";
        assert!(ask(prompt, &b" YES \n"[..], &mut stderr).unwrap());
        assert_eq!(stderr, b"Archive the release board? [y/N] ");
        assert!(!ask(prompt, &b"\n"[..], io::sink()).unwrap());
    }
}
