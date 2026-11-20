use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use ppn::config::{self, ConfigFile, FileStatus, PlatformDirs, nearest_key};
use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::output::{CommandResult, Render};
use ppn::terminal::{Context, Terminal};
use serde_json::{Map, Value, json};

#[derive(Subcommand)]
pub enum Command {
    /// Show effective settings and their sources.
    #[command(after_help = example_help(&["ppn config show", "ppn config show ui.color --json"]))]
    Show {
        /// Show just this key, such as network or ui.color.
        key: Option<String>,
    },
    /// List configuration files and whether they exist.
    #[command(after_help = example_help(&["ppn config path --json"]))]
    Path,
    /// Validate configuration files.
    #[command(after_help = example_help(&["ppn config check", "ppn config check /etc/pepino/config.toml --json"]))]
    Check {
        /// Validate just this file. Omit it to check the selected configuration files.
        file: Option<PathBuf>,
    },
}

pub fn show(
    context: &Context,
    key: Option<&str>,
    files: Vec<FileStatus>,
) -> Result<CommandResult, CliError> {
    let config = context.config.as_ref().ok_or_else(|| {
        CliError::new(
            ErrorKind::Failure,
            "Effective configuration is unavailable.",
        )
    })?;
    let mut values = config.values();
    if let Some(key) = key {
        let Some(value) = values.remove_entry(key) else {
            let error = CliError::new(
                ErrorKind::NotFound,
                format!("Unknown configuration key `{key}`."),
            );
            return Err(match nearest_key(key, values.keys().copied()) {
                Some(candidate) => error.with_hint(format!("Did you mean `{candidate}`?")),
                None => error,
            });
        };
        values = BTreeMap::from([value]);
    }
    Ok(CommandResult::document(Show { values, files }))
}

struct Show {
    values: BTreeMap<&'static str, Value>,
    files: Vec<FileStatus>,
}

impl Render for Show {
    fn human(&self, _: &Terminal) -> String {
        let mut rows = vec![("KEY", "VALUE".into(), "SOURCE".into())];
        rows.extend(self.values.iter().map(|(&key, entry)| {
            let value = display_value(&entry["value"]);
            let source = display_value(&entry["source"]);
            let source = match entry.get("path") {
                Some(path) => format!("{source} ({})", display_value(path)),
                None => source,
            };
            (key, value, source)
        }));
        let key_width = rows.iter().map(|row| row.0.len()).max().unwrap_or(0);
        let value_width = rows
            .iter()
            .map(|row| row.1.chars().count())
            .max()
            .unwrap_or(0);
        rows.into_iter()
            .map(|(key, value, source)| {
                format!("{key:<key_width$}  {value:<value_width$}  {source}\n")
            })
            .collect()
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("values".into(), json!(self.values)),
            ("files".into(), json!(self.files)),
        ])
    }
}

fn display_value(value: &Value) -> String {
    match value.as_str() {
        Some(text) if !text.chars().any(char::is_control) => text.to_owned(),
        _ => value.to_string(),
    }
}

pub fn path(_context: &Context, files: &[ConfigFile]) -> Result<CommandResult, CliError> {
    config::paths(files, Path::try_exists).map(|files| CommandResult::document(PathList(files)))
}

struct PathList(Vec<FileStatus>);

impl Render for PathList {
    fn human(&self, _: &Terminal) -> String {
        self.0
            .iter()
            .map(|file| {
                let status = if file.exists { "present" } else { "missing" };
                format!("{status}  {}\n", file.path.display())
            })
            .collect()
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([("files".into(), json!(self.0))])
    }
}

pub fn check(
    _context: &Context,
    files: &[ConfigFile],
    dirs: &PlatformDirs,
) -> Result<CommandResult, CliError> {
    config::check(files, |path| fs::read_to_string(path), dirs)
        .map(|files| CommandResult::document(Check { files }))
}

struct Check {
    files: Vec<FileStatus>,
}

impl Render for Check {
    fn human(&self, _: &Terminal) -> String {
        let mut output: String = self
            .files
            .iter()
            .filter(|file| file.exists)
            .map(|file| format!("Valid  {}\n", file.path.display()))
            .collect();
        if output.is_empty() {
            output.push_str("No configuration files found.\n");
        }
        output
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("valid".into(), json!(true)),
            ("files".into(), json!(self.files)),
        ])
    }
}
