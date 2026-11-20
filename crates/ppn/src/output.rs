use std::io::{self, Write};
use std::process::ExitCode;

use pepino_app::Response;
use serde_json::{Map, Value, json};

use crate::error::{CliError, ErrorKind};
use crate::terminal::{Context, RenderSettings, Terminal};

pub const SCHEMA_VERSION: u32 = 1;

pub trait Render {
    fn human(&self, terminal: &Terminal) -> String;
    fn json_fields(self) -> Map<String, Value>;
}

type WriteOutput = dyn FnOnce(bool, &Terminal, &mut dyn Write) -> Result<(), CliError>;

pub struct CommandResult(Box<WriteOutput>, Vec<String>);

impl CommandResult {
    pub fn document(value: impl Render + 'static) -> Self {
        Self::stream(std::iter::once(Ok(value)))
    }

    pub fn stream<T: Render + 'static>(
        rows: impl Iterator<Item = Result<T, CliError>> + 'static,
    ) -> Self {
        Self(
            Box::new(move |json, terminal, stdout| {
                for row in rows {
                    if !flush_output(write_row(row?, json, terminal, stdout), stdout)? {
                        break;
                    }
                }
                Ok(())
            }),
            Vec::new(),
        )
    }
    pub fn app_document(
        context: &Context<'_>,
        response: Response,
        human: impl FnOnce(&Response, &RenderSettings) -> Result<String, CliError> + 'static,
    ) -> Result<Self, CliError> {
        let settings = context.render_settings()?;
        Ok(Self(
            Box::new(move |json, _, stdout| {
                let result = if json {
                    match serde_json::to_value(response) {
                        Ok(Value::Object(fields)) => write_json(fields, stdout),
                        Ok(_) => Err(io::Error::other("Expected an app response object.")),
                        Err(error) => Err(error.into()),
                    }
                } else {
                    write_text(&human(&response, &settings)?, stdout)
                };
                flush_output(result, stdout).map(|_| ())
            }),
            Vec::new(),
        ))
    }
    pub fn with_warnings(mut self, warnings: impl IntoIterator<Item = String>) -> Self {
        self.1.extend(warnings);
        self
    }
}

pub fn finish(
    result: Result<CommandResult, CliError>,
    json: bool,
    terminal: &Terminal,
) -> ExitCode {
    finish_to(
        result,
        json,
        terminal,
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    )
}

fn finish_to(
    result: Result<CommandResult, CliError>,
    json: bool,
    terminal: &Terminal,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> ExitCode {
    let result = result.and_then(|CommandResult(write, warnings)| {
        write(json, terminal, stdout)?;
        if !json {
            for warning in warnings {
                let _ = writeln!(stderr, "warning: {warning}");
            }
        }
        Ok(())
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => error.report(json, stderr),
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
    fn emits_success_warnings_to_human_stderr() {
        let terminal = Terminal::fixed(None, false);
        for json in [false, true] {
            let result = CommandResult::document(Task {
                title: "Order a replacement screw",
            })
            .with_warnings(["The dependency would create a cycle.".into()]);
            let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
            assert_eq!(
                finish_to(Ok(result), json, &terminal, &mut stdout, &mut stderr),
                ExitCode::SUCCESS
            );
            assert!(!stdout.is_empty());
            assert_eq!(
                stderr,
                if json {
                    b"".as_slice()
                } else {
                    b"warning: The dependency would create a cycle.\n"
                }
            );
        }
    }

    #[test]
    fn outputs_the_protocol_object_with_only_the_schema_added() {
        let fixture = json!({"kind":"tasks","tasks":[],"total":0,"filter":"due:tomorrow",
            "sort":"due","resolved_dates":{"tomorrow":"2026-11-21"}});
        let response = serde_json::from_value(fixture.clone()).unwrap();
        let context = Context::new(
            Terminal::fixed(None, false),
            false,
            None,
            crate::app::InvocationTime {
                today: "2026-11-20".parse().unwrap(),
                now_ms: 1_795_149_900_000,
                offset: chrono::FixedOffset::west_opt(18_000).unwrap(),
            },
            || {
                use crate::config::{Config, LoadedConfig, Platform, PlatformDirs};
                let dirs = PlatformDirs::new(
                    Platform::Linux,
                    std::path::Path::new("/home/ada"),
                    None,
                    None,
                );
                Ok(LoadedConfig {
                    config: Config::defaults(&dirs).unwrap(),
                    files: vec![],
                })
            },
        );
        let settings = context.render_settings().unwrap();
        assert_eq!(
            settings.dates().calendar("2026-11-21".parse().unwrap()),
            "2026-11-21"
        );
        assert!(!settings.terminal.color);
        let output =
            CommandResult::app_document(&context, response, |_, _| panic!("JSON rendered as text"))
                .unwrap();
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
