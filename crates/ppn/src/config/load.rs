use std::io;
use std::path::Path;

use super::diagnostics::invalid_config;
use super::validate::Draft;
use super::{
    Config, ConfigFile, Env, FileStatus, Flags, PlatformDirs, Setting, Source, select_files,
    validate_file,
};
use crate::error::CliError;

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: Config,
    pub files: Vec<FileStatus>,
}

pub fn load(
    flags: &Flags,
    env: &Env,
    read: impl FnMut(&Path) -> io::Result<String>,
    dirs: &PlatformDirs,
) -> Result<LoadedConfig, CliError> {
    let mut draft = Draft::new(dirs)?;
    let files = visit_files(&select_files(flags, env, dirs), read, |file, text| {
        draft.apply_file(&file.path, text, &file.source)
    })?;
    super::env::apply(&mut draft, env)?;
    if let Some(value) = flags.color {
        draft.config.ui.color = Setting {
            value,
            source: Source::Flag,
        };
    }
    Ok(LoadedConfig {
        config: draft.finish()?,
        files,
    })
}

pub fn check(
    files: &[ConfigFile],
    read: impl FnMut(&Path) -> io::Result<String>,
    dirs: &PlatformDirs,
) -> Result<Vec<FileStatus>, CliError> {
    visit_files(files, read, |file, text| {
        validate_file(&file.path, text, dirs).map(|_| ())
    })
}

fn visit_files(
    files: &[ConfigFile],
    mut read: impl FnMut(&Path) -> io::Result<String>,
    mut visit: impl FnMut(&ConfigFile, &str) -> Result<(), CliError>,
) -> Result<Vec<FileStatus>, CliError> {
    let mut problems = Vec::new();
    let mut statuses = Vec::new();
    for file in files {
        let read_result = match read(&file.path) {
            Ok(text) => {
                if let Err(error) = visit(file, &text) {
                    problems.push(error.to_string());
                }
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        };
        statuses.extend(file.record(read_result, &mut problems));
    }
    invalid_config(problems)?;
    Ok(statuses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Platform, SYSTEM_CONFIG};

    fn absent() -> io::Error {
        io::ErrorKind::NotFound.into()
    }

    fn dirs() -> PlatformDirs {
        PlatformDirs::new(Platform::Linux, Path::new("/home/release"), None, None).unwrap()
    }

    #[test]
    fn merges_files_per_key_and_replaces_lists() {
        let dirs = dirs();
        let loaded = load(&Flags::default(), &Env::new(), |path| {
            if path == Path::new(SYSTEM_CONFIG) {
                Ok("network = 'system-release'\nmdns = false\nrelay_urls = ['https://use.release.example', 'https://eu.release.example']".into())
            } else {
                assert_eq!(path, dirs.user_config.as_path());
                Ok("network = 'user-release'\nrelay_urls = ['https://local.release.example']".into())
            }
        }, &dirs).unwrap();
        let values = loaded.config.values();
        assert_eq!(values["network"]["value"], "user-release");
        assert_eq!(values["network"]["source"], "user_file");
        assert_eq!(
            values["network"]["path"],
            dirs.user_config.as_path().to_str().unwrap()
        );
        assert_eq!(values["mdns"]["value"], false);
        assert_eq!(values["mdns"]["source"], "system_file");
        assert_eq!(values["mdns"]["path"], SYSTEM_CONFIG);
        assert_eq!(values["ui.color"]["source"], "default");
        assert_eq!(
            values["relay_urls"]["value"],
            serde_json::json!(["https://local.release.example"])
        );
        assert!(loaded.files.iter().all(|file| file.exists));
    }

    #[test]
    fn only_default_files_may_be_absent() {
        let dirs = dirs();
        let flags = Flags::default();
        let env = Env::new();
        let loaded = load(&flags, &env, |_| Err(absent()), &dirs).unwrap();
        assert_eq!(loaded.config.network.value.as_str(), "pepino");
        assert!(loaded.files.iter().all(|file| !file.exists));

        let flags = Flags {
            config: Some("/srv/pepino/custom.toml".into()),
            ..flags
        };
        let read = |path: &Path| {
            assert_eq!(path, Path::new("/srv/pepino/custom.toml"));
            Err(absent())
        };
        let error = load(&flags, &env, read, &dirs).unwrap_err();
        assert!(error.to_string().contains("/srv/pepino/custom.toml"));
        let error = check(&select_files(&flags, &env, &dirs), read, &dirs).unwrap_err();
        assert!(error.to_string().contains("/srv/pepino/custom.toml"));
    }
}
