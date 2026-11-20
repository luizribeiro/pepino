use std::env;
use std::io::{self, BufRead, IsTerminal, Write};
use std::time::Duration;

use clap::ValueEnum;

use crate::error::{CliError, ErrorKind};

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum Color {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Debug)]
pub struct Terminal {
    pub stdout_is_tty: bool,
    pub stdin_is_tty: bool,
    pub color: bool,
}

impl Terminal {
    pub fn detect(color: Color) -> Self {
        let stdout_is_tty = io::stdout().is_terminal();
        let stdin_is_tty = io::stdin().is_terminal();
        let no_color = env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
        let dumb = env::var_os("TERM").is_some_and(|value| value == "dumb");
        Self {
            stdout_is_tty,
            stdin_is_tty,
            color: use_color(color, stdout_is_tty, no_color, dumb),
        }
    }
}

#[derive(Debug)]
pub struct Context {
    pub terminal: Terminal,
    pub yes: bool,
    pub timeout: Option<Duration>,
}

impl Context {
    pub fn detect(color: Color, yes: bool, timeout: Option<Duration>) -> Self {
        Self {
            terminal: Terminal::detect(color),
            yes,
            timeout,
        }
    }

    pub fn confirm(&self, prompt: &str) -> Result<bool, CliError> {
        if self.yes {
            return Ok(true);
        }
        if !self.terminal.stdin_is_tty {
            return Err(CliError::new(ErrorKind::ConfirmationRequired, prompt)
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
    fn asks_on_stderr_and_accepts_explicit_yes() {
        let mut stderr = Vec::new();
        let prompt = "Archive the release board?";
        assert!(ask(prompt, &b" YES \n"[..], &mut stderr).unwrap());
        assert_eq!(stderr, b"Archive the release board? [y/N] ");
        assert!(!ask(prompt, &b"\n"[..], io::sink()).unwrap());
    }
}
