use std::collections::BTreeMap;
use std::io;
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

pub fn help_footer() -> String {
    let variables: Vec<_> = config::environment_variables().collect();
    let variable_width = variables
        .iter()
        .map(|entry| entry.variable.len())
        .max()
        .unwrap_or(0);
    let key_width = variables
        .iter()
        .map(|entry| entry.key.len())
        .max()
        .unwrap_or(0);
    let rows: String = variables
        .iter()
        .map(|entry| {
            format!(
                "  {:<variable_width$}  {:<key_width$}  {}\n",
                entry.variable, entry.key, entry.value
            )
        })
        .collect();
    format!(
        "{}\n\nConfiguration environment variables (empty values are unset):\n{rows}",
        example_help(&["ppn config show --json"]),
    )
}

pub fn show(context: &Context<'_>, key: Option<&str>) -> Result<CommandResult, CliError> {
    let loaded = context.config()?;
    let mut values = loaded.config.values();
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
    Ok(CommandResult::document(Show {
        values,
        files: loaded.files.clone(),
    }))
}

struct Show {
    values: BTreeMap<&'static str, Value>,
    files: Vec<FileStatus>,
}

impl Render for Show {
    fn human(&self, _: &Terminal) -> String {
        let mut rows = vec![("KEY", "SOURCE".into(), "VALUE".into())];
        rows.extend(self.values.iter().map(|(&key, entry)| {
            let value = display_value(&entry["value"]);
            let source = display_value(&entry["source"]);
            let source = match entry.get("path") {
                Some(path) => format!("{source} ({})", display_value(path)),
                None => source,
            };
            (key, source, value)
        }));
        let key_width = rows.iter().map(|row| row.0.len()).max().unwrap_or(0);
        let source_width = rows
            .iter()
            .map(|row| row.1.chars().count())
            .max()
            .unwrap_or(0);
        rows.into_iter()
            .map(|(key, source, value)| {
                format!("{key:<key_width$}  {source:<source_width$}  {value}\n")
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

pub fn path(
    files: &[ConfigFile],
    exists: impl FnMut(&Path) -> io::Result<bool>,
) -> Result<CommandResult, CliError> {
    config::paths(files, exists).map(|files| CommandResult::document(PathList(files)))
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
    files: &[ConfigFile],
    dirs: &PlatformDirs,
    read: impl FnMut(&Path) -> io::Result<String>,
) -> Result<CommandResult, CliError> {
    config::check(files, read, dirs).map(|files| CommandResult::document(Check { files }))
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

#[derive(clap::Args)]
#[command(about = "Inspect ppn configuration.", disable_help_subcommand = true, after_help = help_footer())]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

pub fn run(
    context: &Context<'_>,
    inputs: &crate::cli::ProcessInputs,
    config: Option<std::path::PathBuf>,
    args: Args,
) -> Result<CommandResult, CliError> {
    match args.command {
        Command::Show { key } => show(context, key.as_deref()),
        Command::Path => path(&inputs.files(config, None)?, inputs.exists),
        Command::Check { file } => check(
            &inputs.files(config, file.as_deref())?,
            &inputs.dirs,
            inputs.read,
        ),
    }
}
