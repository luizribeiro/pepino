use std::ffi::OsString;

use pepino_app::FilterInput;
use ppn::error::CliError;

pub fn input(text: Option<String>, mut terms: Vec<String>) -> Result<FilterInput, CliError> {
    terms.retain(|term| !term.trim().is_empty());
    match text {
        Some(text) => FilterInput::combine(text, terms)
            .map(|(input, _)| input)
            .map_err(Into::into),
        None => Ok(FilterInput::Argv(terms)),
    }
}

pub fn escape_terms(args: &[OsString]) -> Vec<OsString> {
    let mut args = args.to_vec();
    let mut path = Vec::new();
    let mut option_value = false;
    let mut options = true;
    for arg in args.iter_mut().skip(1) {
        if std::mem::take(&mut option_value) {
            continue;
        }
        let Some(value) = arg.to_str() else { continue };
        if options {
            match value {
                "--" => {
                    options = false;
                    continue;
                }
                "--config" | "--color" | "--timeout" | "--sort" | "--limit" | "--board"
                | "--filter" | "-f" => {
                    option_value = true;
                    continue;
                }
                "-h" | "-V" => continue,
                flag if flag.starts_with("--") || flag.starts_with("-f=") => continue,
                _ => {}
            }
        }
        match path.as_slice() {
            [] if matches!(value, "ls" | "list" | "ready" | "view") => path.push(value.to_owned()),
            [view] if view == "view" && value == "save" => path.push(value.to_owned()),
            [view, _] if view == "view" => path.push(value.to_owned()),
            [command, ..] if command != "view" || path.len() == 3 => {
                // A positional allowing hyphens would also consume subsequent flags.
                if value.starts_with(' ') || (options && value.starts_with('-')) {
                    *arg = format!(" {value}").into();
                }
            }
            _ => break,
        }
    }
    args
}

pub fn unescape_term(value: &str) -> Result<String, std::convert::Infallible> {
    Ok(value.strip_prefix(' ').unwrap_or(value).to_owned())
}
