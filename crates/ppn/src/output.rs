use std::io::{self, Write};
use std::process::ExitCode;

use pepino_app::Response;
use serde_json::{Map, Value, json};

use crate::error::{CliError, ErrorKind};
use crate::terminal::{Color, Terminal};

pub const SCHEMA_VERSION: u32 = 1;

pub trait Render {
    fn human(&self, terminal: &Terminal) -> String;
    fn json_fields(self) -> Map<String, Value>;
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
                if !flush_output(write_row(row?, json, terminal, stdout), stdout)? {
                    break;
                }
            }
            Ok(())
        }))
    }
    pub fn app_document(
        response: Response,
        color: Color,
        human: impl FnOnce(&Response, &Terminal) -> Result<String, CliError> + 'static,
    ) -> Self {
        Self(Box::new(move |json, terminal, stdout| {
            let result = if json {
                match serde_json::to_value(response) {
                    Ok(Value::Object(fields)) => write_json(fields, stdout),
                    Ok(_) => Err(io::Error::other("Expected an app response object.")),
                    Err(error) => Err(error.into()),
                }
            } else {
                write_text(&human(&response, &terminal.task_output(color))?, stdout)
            };
            flush_output(result, stdout).map(|_| ())
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

fn flush_output(result: io::Result<()>, stdout: &mut dyn Write) -> Result<bool, CliError> {
    match result.and_then(|()| stdout.flush()) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(error) => Err(CliError::new(
            ErrorKind::Failure,
            format!("Could not write output: {error}"),
        )),
    }
}

fn write_row(
    value: impl Render,
    json: bool,
    terminal: &Terminal,
    stdout: &mut dyn Write,
) -> io::Result<()> {
    if json {
        write_json(value.json_fields(), stdout)
    } else {
        write_text(&value.human(terminal), stdout)
    }
}

fn write_json(mut object: Map<String, Value>, stdout: &mut dyn Write) -> io::Result<()> {
    debug_assert!(
        !object.contains_key("schema"),
        "The output layer owns the schema field"
    );
    object.insert("schema".into(), json!(SCHEMA_VERSION));
    serde_json::to_writer(&mut *stdout, &object)?;
    writeln!(stdout)
}

fn write_text(text: &str, stdout: &mut dyn Write) -> io::Result<()> {
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

    struct Task {
        title: &'static str,
    }

    impl Render for Task {
        fn human(&self, _: &Terminal) -> String {
            self.title.to_owned()
        }

        fn json_fields(self) -> Map<String, Value> {
            Map::from_iter([("title".into(), json!(self.title))])
        }
    }

    #[test]
    fn outputs_the_protocol_object_with_only_the_schema_added() {
        let fixture = json!({"kind":"tasks","tasks":[],"total":0,"filter":"due:tomorrow",
            "sort":"due","resolved_dates":{"tomorrow":"2026-11-21"}});
        let response = serde_json::from_value(fixture.clone()).unwrap();
        let output = CommandResult::app_document(response, Color::Never, |_, _| {
            panic!("JSON rendered as text")
        });
        let mut stdout = Vec::new();
        (output.0)(true, &Terminal::fixed(None, false), &mut stdout).unwrap();
        let mut expected = fixture;
        expected["schema"] = json!(1);
        assert_eq!(serde_json::from_slice::<Value>(&stdout).unwrap(), expected);
    }

    #[test]
    fn writes_one_versioned_document_per_stream_row() {
        let rows = ["Ship the release", "Notify the mailing list"]
            .into_iter()
            .map(|title| Ok(Task { title }));
        let mut stdout = Vec::new();
        (CommandResult::stream(rows).0)(
            true,
            &Terminal::detect(Color::Never, &Default::default()),
            &mut stdout,
        )
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
        let error = (command.0)(
            true,
            &Terminal::detect(Color::Never, &Default::default()),
            &mut stdout,
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::DaemonUnreachable);
        assert_eq!(
            stdout.flushed,
            b"{\"schema\":1,\"title\":\"Ship the release\"}\n"
        );
        assert!(stdout.buffered.is_empty());
    }
}
