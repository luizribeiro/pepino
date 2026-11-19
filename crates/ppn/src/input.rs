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

#[cfg(test)]
mod tests {
    use super::*;

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
