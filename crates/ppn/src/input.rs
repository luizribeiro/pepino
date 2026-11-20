pub mod form;

use std::fs;
use std::io;
use std::path::Path;

use crate::error::{CliError, ErrorKind};

pub fn read_text(value: Option<String>, from_file: Option<&Path>) -> Result<String, CliError> {
    match (value, from_file) {
        (Some(_), Some(_)) => Err(CliError::new(
            ErrorKind::Usage,
            "Pass either a text argument or --from-file.",
        )),
        (None, None) => Err(
            CliError::new(ErrorKind::Usage, "A text argument is required.")
                .with_hint("Pass text, - to read stdin, or --from-file <path>."),
        ),
        (None, Some(path)) => {
            fs::read_to_string(path).map_err(|error| read_error(&path.display().to_string(), error))
        }
        (Some(text), None) if text == "-" => {
            io::read_to_string(io::stdin().lock()).map_err(|error| read_error("stdin", error))
        }
        (Some(text), None) => Ok(text),
    }
}

fn read_error(source: &str, error: io::Error) -> CliError {
    let kind = match error.kind() {
        io::ErrorKind::NotFound => ErrorKind::NotFound,
        io::ErrorKind::PermissionDenied => ErrorKind::PermissionDenied,
        _ => ErrorKind::Failure,
    };
    CliError::new(kind, format!("Could not read {source}: {error}"))
}

#[cfg(unix)]
pub fn edit_text<T>(
    text: &str,
    json: bool,
    parse: impl Fn(&str) -> Result<T, CliError>,
) -> Result<T, CliError> {
    let editor = ["VISUAL", "EDITOR"]
        .into_iter()
        .filter_map(std::env::var_os)
        .find(|value| !value.to_string_lossy().trim().is_empty())
        .ok_or_else(|| {
            CliError::new(ErrorKind::Usage, "No editor is configured.")
                .with_hint("Set VISUAL or EDITOR, or use the edit flags.")
        })?;
    run_editor(&editor, text, json, parse)
}

#[cfg(unix)]
fn run_editor<T>(
    editor: &std::ffi::OsStr,
    text: &str,
    json: bool,
    parse: impl Fn(&str) -> Result<T, CliError>,
) -> Result<T, CliError> {
    use std::ffi::OsString;
    use std::process::{Command, Stdio};

    let directory = tempfile::tempdir().map_err(|error| read_error("editor directory", error))?;
    let file = directory.path().join("task.json");
    fs::write(&file, text).map_err(|error| read_error("editor file", error))?;
    let mut command = if Path::new(editor).is_file() {
        Command::new(editor)
    } else {
        let mut command = Command::new("sh");
        let mut shell_command = OsString::from("exec ");
        shell_command.push(editor);
        shell_command.push(r#" "$1""#);
        command.arg("-c").arg(shell_command).arg("ppn-editor");
        command
    };
    let output = fs::OpenOptions::new()
        .write(true)
        .open("/dev/tty")
        .map_or_else(|_| Stdio::from(io::stderr()), Stdio::from);
    command.arg(&file).stdout(output);
    loop {
        let status = command.status().map_err(|error| {
            CliError::new(
                ErrorKind::Failure,
                format!("Could not start the editor: {error}"),
            )
        })?;
        if !status.success() {
            return Err(CliError::new(
                ErrorKind::Failure,
                format!("The editor exited unsuccessfully ({status})."),
            ));
        }
        let contents =
            fs::read_to_string(&file).map_err(|error| read_error("editor file", error))?;
        match parse(&contents) {
            Ok(value) => return Ok(value),
            Err(error) => {
                error.report(json, io::stderr().lock());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn editor_receives_a_file_and_reports_unsuccessful_exit() {
        use std::ffi::OsStr;

        let title = "Order stainless fittings";
        let command = r#"sh -c 'test "$(cat "$1")" = "Order stainless fittings" && printf "%s" "Order brass fittings" > "$1"' editor"#;
        assert_eq!(
            run_editor(
                OsStr::new(command),
                title,
                false,
                |text| Ok(text.to_owned())
            )
            .unwrap(),
            "Order brass fittings"
        );
        let error = run_editor(
            OsStr::new("false"),
            title,
            false,
            |text| Ok(text.to_owned()),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Failure);
    }

    #[test]
    fn keeps_literal_text_intact() {
        let text = "Ship release notes\nCoordinate with José";
        assert_eq!(read_text(Some(text.into()), None).unwrap(), text);
        assert_eq!(read_text(Some(String::new()), None).unwrap(), "");
    }

    #[test]
    fn rejects_missing_or_conflicting_sources() {
        assert_eq!(read_text(None, None).unwrap_err().kind(), ErrorKind::Usage);
        assert_eq!(
            read_text(
                Some("Ship release notes".into()),
                Some(Path::new("notes.txt"))
            )
            .unwrap_err()
            .kind(),
            ErrorKind::Usage
        );
    }
}
