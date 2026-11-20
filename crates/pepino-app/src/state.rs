use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use pepino_core::{BoardId, DeviceId, IdentityId};
use serde::{Deserialize, Serialize};

use crate::AppError;
use crate::app::invalid;
use crate::views::SavedViews;

const VERSION: u32 = 2;
pub(super) const FILE_NAME: &str = "local.json";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenOptions {
    pub display_name: Option<String>,
    pub device_name: Option<String>,
    pub board_name: Option<String>,
    /// Maximum wait for the data directory lock on each open or request.
    pub lock_timeout: Duration,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            display_name: None,
            device_name: None,
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
    pub(crate) saved_views: SavedViews,
}

impl LocalState {
    pub(super) fn task_names<'a>(
        &self,
        tasks: impl IntoIterator<Item = &'a crate::TaskSummary>,
    ) -> std::collections::BTreeMap<String, String> {
        self.names_for(tasks.into_iter().flat_map(|task| {
            std::iter::once(task.author.as_str()).chain(task.assignees.iter().map(String::as_str))
        }))
    }

    pub(super) fn names_for<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a str>,
    ) -> std::collections::BTreeMap<String, String> {
        let identity = self.identity_id.to_string();
        let device = self.device_id.to_string();
        let mut names = std::collections::BTreeMap::new();
        for id in ids {
            if id == identity {
                names
                    .entry(identity.clone())
                    .or_insert_with(|| self.display_name.clone());
            } else if id == device {
                names
                    .entry(device.clone())
                    .or_insert_with(|| self.device_name.clone());
            }
        }
        names
    }

    pub(super) fn new(
        names: InitialNames,
        device_id: DeviceId,
        board_id: BoardId,
    ) -> Result<Self, AppError> {
        let InitialNames {
            display_name,
            board_name,
            device_name,
        } = names;
        Ok(Self {
            version: VERSION,
            device_id,
            identity_id: IdentityId::from_bytes(random_id()?),
            display_name,
            device_name,
            board_id,
            board_name,
            saved_views: SavedViews::default(),
        })
    }

    pub(crate) fn load_or_create(directory: &Path, options: OpenOptions) -> Result<Self, AppError> {
        let path = directory.join(FILE_NAME);
        match fs::read(&path) {
            Ok(bytes) => return Self::decode(&bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(path_io_error(&path, "read local state", error)),
        }
        let state = Self::new(
            InitialNames::try_from(options)?,
            DeviceId::from_bytes(random_id()?),
            BoardId::from_bytes(random_id()?),
        )?;
        save(directory, &path, state)
            .map_err(|error| path_io_error(&path, "save local state", error))
    }

    pub(crate) fn load(directory: &Path) -> Result<Self, AppError> {
        let path = directory.join(FILE_NAME);
        let bytes =
            fs::read(&path).map_err(|error| path_io_error(&path, "read local state", error))?;
        Self::decode(&bytes)
    }

    pub(super) fn migrate(mut self, directory: &Path) -> Result<Self, AppError> {
        if self.version != VERSION {
            self.version = VERSION;
            self.replace(directory)?;
        }
        Ok(self)
    }

    pub(super) fn prepare_directory(directory: &Path) -> Result<(), AppError> {
        create_directory(directory)
            .map_err(|error| path_io_error(directory, "create data directory", error))
    }

    pub(crate) fn replace(&self, directory: &Path) -> Result<(), AppError> {
        let replace = || {
            stage(directory, self)?
                .persist(directory.join(FILE_NAME))
                .map_err(|error| error.error)?;
            sync_directory(directory)
        };
        replace()
            .map_err(|error| path_io_error(&directory.join(FILE_NAME), "save local state", error))
    }

    fn decode(bytes: &[u8]) -> Result<Self, AppError> {
        #[derive(Deserialize)]
        struct Version {
            version: u64,
        }

        let Version { version } = serde_json::from_slice(bytes)
            .map_err(|error| storage("decode local state", error, false))?;
        match version {
            1 => {
                let mut state: serde_json::Value = serde_json::from_slice(bytes)
                    .map_err(|error| storage("decode local state", error, false))?;
                state["saved_views"] = serde_json::json!({});
                serde_json::from_value(state)
                    .map_err(|error| storage("decode local state", error, false))
            }
            version if version == u64::from(VERSION) => serde_json::from_slice(bytes)
                .map_err(|error| storage("decode local state", error, false)),
            version => Err(storage("unsupported local state version", version, false)),
        }
    }
}

pub(super) struct InitialNames {
    display_name: String,
    device_name: String,
    board_name: String,
}

impl TryFrom<OpenOptions> for InitialNames {
    type Error = AppError;

    fn try_from(options: OpenOptions) -> Result<Self, AppError> {
        let display_name = match options.display_name {
            Some(name) => name,
            None => {
                whoami::username().map_err(|error| storage("get OS user name", error, false))?
            }
        };
        let board_name = options.board_name.unwrap_or_else(|| "Personal".into());
        let device_name = match options.device_name {
            Some(name) => name,
            None => whoami::hostname().map_err(|error| storage("get hostname", error, false))?,
        };
        for (field, value) in [
            ("display_name", &display_name),
            ("device_name", &device_name),
            ("board_name", &board_name),
        ] {
            if value.trim().is_empty() {
                return Err(invalid(
                    field,
                    "name must contain a non-whitespace character",
                ));
            }
        }
        Ok(Self {
            display_name,
            device_name,
            board_name,
        })
    }
}

fn create_directory(directory: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory)
}

fn save(directory: &Path, path: &Path, state: LocalState) -> io::Result<LocalState> {
    create_directory(directory)?;
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

pub(crate) fn path_io_error(path: &Path, action: &str, error: io::Error) -> AppError {
    if matches!(
        error.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
    ) {
        return AppError::PermissionDenied {
            path: path.display().to_string(),
            reason: action.into(),
        };
    }
    io_error(&format!("{action} {}", path.display()), error)
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
            device_name: Some("garden tablet".into()),
            board_name: Some("Greenhouse".into()),
            ..OpenOptions::default()
        };
        let state = LocalState::load_or_create(directory.path(), options).unwrap();
        let path = directory.path().join(FILE_NAME);
        assert_eq!(state.display_name, "Ada Gardener");
        assert_eq!(state.board_name, "Greenhouse");
        assert_eq!(state.device_name, "garden tablet");
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
        assert_eq!(other.device_name, whoami::hostname().unwrap());
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
