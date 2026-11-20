use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use pepino_core::{BoardId, DeviceId, IdentityId};
use serde::{Deserialize, Serialize};

use crate::AppError;

const VERSION: u32 = 1;
pub(crate) const FILE_NAME: &str = "local.json";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenOptions {
    pub display_name: Option<String>,
    pub board_name: Option<String>,
    /// Maximum wait for the data directory lock on each open or request.
    pub lock_timeout: Duration,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            display_name: None,
            board_name: None,
            lock_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LocalState {
    version: u32,
    pub device_id: DeviceId,
    pub identity_id: IdentityId,
    pub display_name: String,
    pub device_name: String,
    pub board_id: BoardId,
    pub board_name: String,
}

impl LocalState {
    pub(crate) fn load_or_create(directory: &Path, options: OpenOptions) -> Result<Self, AppError> {
        let path = directory.join(FILE_NAME);
        match fs::read(&path) {
            Ok(bytes) => return Self::decode(&bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("read local state", error)),
        }
        let display_name = match options.display_name {
            Some(name) => name,
            None => {
                whoami::username().map_err(|error| storage("get OS user name", error, false))?
            }
        };
        let board_name = options.board_name.unwrap_or_else(|| "Personal".into());
        for (field, value) in [("display_name", &display_name), ("board_name", &board_name)] {
            if value.trim().is_empty() {
                return Err(AppError::InvalidInput {
                    field: field.into(),
                    reason: "name must contain a non-whitespace character".into(),
                });
            }
        }
        let device_name =
            whoami::hostname().map_err(|error| storage("get hostname", error, false))?;
        let state = Self {
            version: VERSION,
            device_id: DeviceId::from_bytes(random_id()?),
            identity_id: IdentityId::from_bytes(random_id()?),
            display_name,
            device_name,
            board_id: BoardId::from_bytes(random_id()?),
            board_name,
        };
        save(directory, &path, state).map_err(|error| io_error("save local state", error))
    }

    fn decode(bytes: &[u8]) -> Result<Self, AppError> {
        #[derive(Deserialize)]
        struct Version {
            version: u64,
        }

        let Version { version } = serde_json::from_slice(bytes)
            .map_err(|error| storage("decode local state", error, false))?;
        if version != u64::from(VERSION) {
            return Err(storage("unsupported local state version", version, false));
        }
        serde_json::from_slice(bytes).map_err(|error| storage("decode local state", error, false))
    }
}

fn save(directory: &Path, path: &Path, state: LocalState) -> io::Result<LocalState> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory)?;
    let file = stage(directory, &state)?;
    match file.persist_noclobber(path) {
        Ok(_) => {
            sync_directory(directory)?;
            Ok(state)
        }
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
            let bytes = fs::read(path)?;
            LocalState::decode(&bytes).map_err(io::Error::other)
        }
        Err(error) => Err(error.error),
    }
}

fn stage(directory: &Path, state: &LocalState) -> io::Result<tempfile::NamedTempFile> {
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    serde_json::to_writer(file.as_file_mut(), state)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    Ok(file)
}

fn sync_directory(_directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(_directory)?.sync_all()?;
    Ok(())
}

fn random_id() -> Result<[u8; 32], AppError> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|error| storage("generate local ID", error, false))?;
    Ok(bytes)
}

pub(crate) fn storage(context: &str, error: impl std::fmt::Display, retryable: bool) -> AppError {
    AppError::Storage {
        reason: format!("{context}: {error}"),
        retryable,
    }
}

pub(crate) fn io_error(context: &str, error: io::Error) -> AppError {
    let retryable = matches!(
        error.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    );
    storage(context, error, retryable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_persists_private_state_and_preserves_first_names() {
        let directory = tempfile::tempdir().unwrap();
        let options = OpenOptions {
            display_name: Some("Ada Gardener".into()),
            board_name: Some("Greenhouse".into()),
            ..OpenOptions::default()
        };
        let state = LocalState::load_or_create(directory.path(), options).unwrap();
        let path = directory.path().join(FILE_NAME);
        assert_eq!(state.display_name, "Ada Gardener");
        assert_eq!(state.board_name, "Greenhouse");
        assert_eq!(state.device_name, whoami::hostname().unwrap());
        assert_eq!(
            LocalState::load_or_create(directory.path(), OpenOptions::default()).unwrap(),
            state
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let other = tempfile::tempdir().unwrap();
        let other = LocalState::load_or_create(other.path(), OpenOptions::default()).unwrap();
        assert_eq!(other.display_name, whoami::username().unwrap());
        assert_eq!(other.board_name, "Personal");
        assert_ne!(state.device_id, other.device_id);
        assert_ne!(state.identity_id, other.identity_id);
        assert_ne!(state.board_id, other.board_id);
        fs::write(
            path,
            br#"{"version":4294967296,"credentials":{"device":"future"}}"#,
        )
        .unwrap();
        assert!(matches!(
            LocalState::load_or_create(directory.path(), OpenOptions::default()),
            Err(AppError::Storage {
                retryable: false,
                reason,
            }) if reason == "unsupported local state version: 4294967296"
        ));
    }

    #[test]
    fn racing_creators_use_the_same_state() {
        let directory = tempfile::tempdir().unwrap();
        let start = std::sync::Barrier::new(2);
        let create = |name: &str| {
            start.wait();
            LocalState::load_or_create(
                directory.path(),
                OpenOptions {
                    display_name: Some(name.into()),
                    ..OpenOptions::default()
                },
            )
            .unwrap()
        };
        let [left, right] = std::thread::scope(|scope| {
            let left = scope.spawn(|| create("Ada Gardener"));
            let right = scope.spawn(|| create("Ben Gardener"));
            [left.join().unwrap(), right.join().unwrap()]
        });
        assert_eq!(left, right);
    }
}
