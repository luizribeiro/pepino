use std::ffi::OsString;

use pepino_app::{FilterInput, FilterPosition};
use ppn::error::CliError;

#[derive(Default)]
pub struct Positions {
    arguments: Vec<(usize, usize)>,
    appended: Vec<usize>,
}

impl Positions {
    pub fn remap(&self, error: CliError, view: Option<&str>) -> CliError {
        error.remap_filter_position(view, |position| match position {
            FilterPosition::Argument { index, byte } => {
                let &(index, _) = self.arguments.get(index)?;
                Some(FilterPosition::Argument { index, byte })
            }
            FilterPosition::Byte { byte } => {
                let argument = self.appended.binary_search(&byte).ok()?;
                let &(index, byte) = self.arguments.get(argument)?;
                Some(FilterPosition::Argument { index, byte })
            }
            _ => None,
        })
    }
}

pub fn input(text: Option<String>, terms: Vec<String>) -> Result<FilterInput, CliError> {
    input_with_positions(text, terms).map(|(input, _)| input)
}

pub fn input_with_positions(
    text: Option<String>,
    terms: Vec<String>,
) -> Result<(FilterInput, Positions), CliError> {
    let mut positions = Positions::default();
    let mut argv = Vec::new();
    for (index, term) in terms.into_iter().enumerate() {
        if !term.trim().is_empty() {
            positions
                .arguments
                .push((index, term.len() - term.trim_start().len()));
            argv.push(term);
        }
    }
    let Some(text) = text else {
        return Ok((FilterInput::Argv(argv), positions));
    };
    let (input, starts) =
        FilterInput::combine(text, argv).map_err(|error| positions.remap(error.into(), None))?;
    positions.appended = starts;
    Ok((input, positions))
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
