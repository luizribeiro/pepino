use std::io::{self, Write};
use std::process::ExitCode;

use serde::Serialize;
use serde_json::{Value, json};

use crate::error::{CliError, ErrorKind};
use crate::terminal::Terminal;

pub const SCHEMA_VERSION: u32 = 1;

pub trait Render: Serialize {
    fn human(&self, terminal: &Terminal) -> String;
}

type WriteOutput = dyn FnOnce(bool, &Terminal, &mut dyn Write) -> Result<(), CliError>;

pub struct CommandResult(Box<WriteOutput>);

impl CommandResult {
    pub fn document(value: impl Render + 'static) -> Self {
        Self::stream(std::iter::once(Ok(value)))
    }

    pub fn stream<T: Render + 'static>(
        rows: impl Iterator<Item = Result<T, CliError>> + 'static,
    ) -> Self {
        Self(Box::new(move |json, terminal, stdout| {
            for row in rows {
                match write_row(&row?, json, terminal, stdout).and_then(|()| stdout.flush()) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
                    Err(error) => {
                        return Err(CliError::new(
                            ErrorKind::Failure,
                            format!("Could not write output: {error}"),
                        ));
                    }
                }
            }
            Ok(())
        }))
    }
}

pub fn finish(
    result: Result<CommandResult, CliError>,
    json: bool,
    terminal: &Terminal,
) -> ExitCode {
    match result.and_then(|output| (output.0)(json, terminal, &mut io::stdout().lock())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => error.report(json, io::stderr().lock()),
    }
}

fn write_row(
    value: &impl Render,
    json: bool,
    terminal: &Terminal,
    stdout: &mut dyn Write,
) -> io::Result<()> {
    if json {
        let Value::Object(mut object) = serde_json::to_value(value)? else {
            return Err(io::Error::other(
                "A command result must serialize as a JSON object",
            ));
        };
        object.insert("schema".into(), json!(SCHEMA_VERSION));
        serde_json::to_writer(&mut *stdout, &object)?;
        return writeln!(stdout);
    }

    let text = value.human(terminal);
    stdout.write_all(text.as_bytes())?;
    if !text.ends_with('\n') {
        writeln!(stdout)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Color;

    #[derive(Serialize)]
    struct Task {
        title: &'static str,
    }

    impl Render for Task {
        fn human(&self, _: &Terminal) -> String {
            self.title.to_owned()
        }
    }

    #[test]
    fn writes_one_versioned_document_per_stream_row() {
        let rows = ["Ship the release", "Notify the mailing list"]
            .into_iter()
            .map(|title| Ok(Task { title }));
        let mut stdout = Vec::new();
        (CommandResult::stream(rows).0)(true, &Terminal::detect(Color::Never), &mut stdout)
            .unwrap();
        assert_eq!(
            String::from_utf8(stdout).unwrap(),
            "{\"schema\":1,\"title\":\"Ship the release\"}\n{\"schema\":1,\"title\":\"Notify the mailing list\"}\n"
        );
    }

    #[test]
    fn preserves_complete_rows_when_the_stream_fails() {
        #[derive(Default)]
        struct Sink {
            buffered: Vec<u8>,
            flushed: Vec<u8>,
        }

        impl Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.buffered.write(bytes)
            }

            fn flush(&mut self) -> io::Result<()> {
                self.flushed.append(&mut self.buffered);
                Ok(())
            }
        }

        let first = Task {
            title: "Ship the release",
        };
        let interrupted = CliError::new(ErrorKind::DaemonUnreachable, "Lost contact during sync");
        let later = Task {
            title: "Notify the mailing list",
        };
        let command = CommandResult::stream([Ok(first), Err(interrupted), Ok(later)].into_iter());
        let mut stdout = Sink::default();
        let error = (command.0)(true, &Terminal::detect(Color::Never), &mut stdout).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::DaemonUnreachable);
        assert_eq!(
            stdout.flushed,
            b"{\"schema\":1,\"title\":\"Ship the release\"}\n"
        );
        assert!(stdout.buffered.is_empty());
    }
}
