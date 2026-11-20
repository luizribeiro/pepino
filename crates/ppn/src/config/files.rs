use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::diagnostics::invalid_config;
use super::path::{expand_home, serialize_path};
use super::{PlatformDirs, SYSTEM_CONFIG, Source};
use crate::error::CliError;
use crate::terminal::Color;

pub const CONFIG_ENV: &str = "PPN_CONFIG";

pub type Env = BTreeMap<OsString, OsString>;

#[derive(Clone, Debug, Default)]
pub struct Flags {
    pub config: Option<PathBuf>,
    pub color: Option<Color>,
}

#[derive(Clone, Debug)]
pub struct ConfigFile {
    pub path: PathBuf,
    pub source: Source,
}

impl ConfigFile {
    pub fn explicit(path: &Path, dirs: &PlatformDirs) -> Self {
        let path = expand_home(path, dirs.home.as_path());
        Self {
            source: Source::ExplicitFile { path: path.clone() },
            path,
        }
    }

    pub(crate) fn required(&self) -> bool {
        matches!(self.source, Source::ExplicitFile { .. })
    }

    pub(crate) fn record(
        &self,
        result: io::Result<bool>,
        problems: &mut Vec<String>,
    ) -> Option<FileStatus> {
        match result {
            Ok(false) if self.required() => {
                problems.push(format!(
                    "{}: configuration file does not exist",
                    self.path.display()
                ));
                None
            }
            Ok(exists) => Some(FileStatus {
                path: self.path.clone(),
                exists,
            }),
            Err(error) => {
                problems.push(format!("{}: {error}", self.path.display()));
                None
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileStatus {
    #[serde(serialize_with = "serialize_path")]
    pub path: PathBuf,
    pub exists: bool,
}

pub fn select_files(flags: &Flags, env: &Env, dirs: &PlatformDirs) -> Vec<ConfigFile> {
    let explicit = flags
        .config
        .as_deref()
        .or_else(|| env.get(OsStr::new(CONFIG_ENV)).map(Path::new));
    if let Some(path) = explicit {
        return vec![ConfigFile::explicit(path, dirs)];
    }

    let system = PathBuf::from(SYSTEM_CONFIG);
    let user = dirs.user_config.as_path().to_owned();
    vec![
        ConfigFile {
            source: Source::SystemFile {
                path: system.clone(),
            },
            path: system,
        },
        ConfigFile {
            source: Source::UserFile { path: user.clone() },
            path: user,
        },
    ]
}

pub fn paths(
    files: &[ConfigFile],
    mut exists: impl FnMut(&Path) -> io::Result<bool>,
) -> Result<Vec<FileStatus>, CliError> {
    let mut problems = Vec::new();
    let statuses = files
        .iter()
        .filter_map(|file| file.record(exists(&file.path), &mut problems))
        .collect();
    invalid_config(problems)?;
    Ok(statuses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Platform;

    #[test]
    fn explicit_selection_replaces_both_default_paths() {
        let dirs =
            PlatformDirs::new(Platform::Linux, Path::new("/home/release"), None, None).unwrap();
        let mut env = Env::new();
        let mut flags = Flags::default();
        let files = select_files(&flags, &env, &dirs);
        let statuses = paths(&files, |path| Ok(path == dirs.user_config.as_path())).unwrap();
        assert_eq!(statuses.len(), 2);
        assert_eq!(
            statuses[0],
            FileStatus {
                path: SYSTEM_CONFIG.into(),
                exists: false
            }
        );
        assert_eq!(
            statuses[1],
            FileStatus {
                path: dirs.user_config.as_path().into(),
                exists: true
            }
        );

        env.insert(CONFIG_ENV.into(), "~/profile.toml".into());
        let files = select_files(&flags, &env, &dirs);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, Path::new("/home/release/profile.toml"));
        let error = paths(&files, |_| Ok(false)).unwrap_err();
        assert!(error.to_string().contains("/home/release/profile.toml"));

        flags.config = Some("/srv/pepino/custom.toml".into());
        let files = select_files(&flags, &env, &dirs);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, Path::new("/srv/pepino/custom.toml"));
        assert_eq!(paths(&files, |_| Ok(true)).unwrap()[0].path, files[0].path);
    }
}
