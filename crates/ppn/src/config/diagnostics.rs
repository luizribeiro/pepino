use std::ops::Range;
use std::path::Path;

use crate::error::{CliError, ErrorKind};

pub fn nearest_key<'a>(key: &str, keys: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    keys.into_iter()
        .min_by_key(|candidate| strsim::damerau_levenshtein(key, candidate))
}

pub(crate) fn invalid_config(problems: Vec<String>) -> Result<(), CliError> {
    if problems.is_empty() {
        return Ok(());
    }
    Err(CliError::new(ErrorKind::InvalidConfig, problems.join("\n")))
}

pub(crate) fn at_location(path: &Path, text: &str, span: Option<Range<usize>>) -> String {
    let path = path.display();
    let Some(span) = span else {
        return path.to_string();
    };
    let prefix = &text[..text.floor_char_boundary(span.start.min(text.len()))];
    let line = prefix.bytes().filter(|&byte| byte == b'\n').count() + 1;
    let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    format!("{path}:{line}:{column}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_transposed_and_nested_keys() {
        assert_eq!(nearest_key("netwrok", ["mdns", "network"]), Some("network"));
        assert_eq!(
            nearest_key("ui.date_fromat", ["ui.color", "ui.date_format"]),
            Some("ui.date_format")
        );
    }

    #[test]
    fn reports_the_file_and_character_position() {
        let path = Path::new("/srv/pepino/config.toml");
        assert_eq!(
            at_location(path, "network='é'\nmdns=2", Some(18..19)),
            "/srv/pepino/config.toml:2:6"
        );
        assert_eq!(at_location(path, "mdns=2", None), "/srv/pepino/config.toml");
        assert!(invalid_config(Vec::new()).is_ok());
        assert_eq!(
            invalid_config(vec!["bad network".into(), "bad relay".into()])
                .unwrap_err()
                .to_string(),
            "bad network\nbad relay"
        );
    }
}
