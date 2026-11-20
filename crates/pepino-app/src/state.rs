use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use pepino_core::{BoardId, DeviceId, IdentityId};
use serde::{Deserialize, Serialize};

use crate::AppError;
use crate::app::invalid;
use crate::views::SavedViews;

pub(super) const VERSION: u32 = 4;
pub(super) const FILE_NAME: &str = "local.json";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenOptions {
    pub display_name: Option<String>,
    pub device_name: Option<String>,
    pub board_name: Option<String>,
    pub credentials_file: Option<PathBuf>,
    /// Maximum wait for the data directory lock on each open or request.
    pub lock_timeout: Duration,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            display_name: None,
            device_name: None,
            board_name: None,
            credentials_file: None,
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
    pub(super) boards: Vec<LocalBoard>,
    pub(crate) saved_views: SavedViews,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LocalBoard {
    pub id: BoardId,
    pub name: String,
}

impl LocalState {
    pub fn board_id(&self) -> BoardId {
        self.boards[0].id
    }

    pub fn board_name(&self) -> &str {
        &self.boards[0].name
    }

    pub fn boards(&self) -> &[LocalBoard] {
        &self.boards
    }

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
            boards: vec![LocalBoard {
                id: board_id,
                name: board_name,
            }],
            saved_views: SavedViews::default(),
        })
    }

    pub(crate) fn load(directory: &Path) -> Result<Self, AppError> {
        Self::read(&directory.join(FILE_NAME))
    }

    pub(super) fn load_migrating(directory: &Path) -> Result<Self, AppError> {
        let path = directory.join(FILE_NAME);
        let bytes =
            fs::read(&path).map_err(|error| path_io_error(&path, "read local state", error))?;
        let local = Self::decode(&bytes, &path)?;
        if serde_json::from_slice::<serde_json::Value>(&bytes)
            .is_ok_and(|value| value["version"] == 3)
        {
            local.write(&path)?;
        }
        Ok(local)
    }

    pub(super) fn read(path: &Path) -> Result<Self, AppError> {
        let bytes =
            fs::read(path).map_err(|error| path_io_error(path, "read local state", error))?;
        Self::decode(&bytes, path)
    }

    pub(super) fn prepare_directory(directory: &Path) -> Result<(), AppError> {
        create_directory(directory)
            .map_err(|error| path_io_error(directory, "create data directory", error))
    }

    pub(crate) fn replace(&self, directory: &Path) -> Result<(), AppError> {
        self.write(&directory.join(FILE_NAME))
    }

    pub(super) fn write(&self, path: &Path) -> Result<(), AppError> {
        let directory = path
            .parent()
            .ok_or_else(|| storage("save local state", "missing parent directory", false))?;
        let replace = || {
            stage(directory, self)?
                .persist(path)
                .map_err(|error| error.error)?;
            sync_directory(directory)
        };
        replace().map_err(|error| path_io_error(path, "save local state", error))
    }

    fn decode(bytes: &[u8], path: &Path) -> Result<Self, AppError> {
        #[derive(Deserialize)]
        struct Version {
            version: u64,
        }

        let decode_error = |error| storage("decode local state", error, false);
        let mut value: serde_json::Value = serde_json::from_slice(bytes).map_err(decode_error)?;
        let Version { version } = serde_json::from_value(value.clone()).map_err(decode_error)?;
        match version {
            1 | 2 => {
                return Err(AppError::IncompatibleLocalState {
                    version,
                    directory: path.parent().unwrap_or(path).display().to_string(),
                });
            }
            3 => {
                let fields = value
                    .as_object_mut()
                    .ok_or_else(|| storage("decode local state", "expected an object", false))?;
                let id = fields.remove("board_id");
                let name = fields.remove("board_name");
                fields.insert(
                    "boards".into(),
                    serde_json::json!([{ "id": id, "name": name }]),
                );
                fields.insert("version".into(), VERSION.into());
            }
            version if version == u64::from(VERSION) => (),
            version => return Err(storage("unsupported local state version", version, false)),
        }
        let local: Self = serde_json::from_value(value).map_err(decode_error)?;
        if local.boards.is_empty() {
            return Err(storage(
                "decode local state",
                "missing current board",
                false,
            ));
        }
        Ok(local)
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

pub(super) fn sync_directory(_directory: &Path) -> io::Result<()> {
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
    fn persists_private_state_with_the_supplied_ids() {
        let directory = tempfile::tempdir().unwrap();
        let names = OpenOptions {
            display_name: Some("Ada Gardener".into()),
            device_name: Some("garden tablet".into()),
            board_name: Some("Greenhouse".into()),
            ..OpenOptions::default()
        }
        .try_into()
        .unwrap();
        let device = DeviceId::from_bytes([13; 32]);
        let board = BoardId::from_bytes([17; 32]);
        let state = LocalState::new(names, device, board).unwrap();
        let path = directory.path().join(FILE_NAME);
        state.write(&path).unwrap();
        let restored = LocalState::load(directory.path()).unwrap();
        assert_eq!(restored, state);
        assert_eq!(restored.device_id, device);
        assert_eq!(restored.board_id(), board);
        assert_eq!(restored.display_name, "Ada Gardener");
        assert_eq!(restored.board_name(), "Greenhouse");
        assert_eq!(restored.device_name, "garden tablet");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn migrates_v3_with_its_board_and_saved_views() {
        use serde_json::json;
        let fields = json!({
            "version": 3,
            "board_id": BoardId::from_bytes([12; 32]),
            "board_name": "Greenhouse",
            "device_id": DeviceId::from_bytes([34; 32]),
            "device_name": "Garden laptop",
            "identity_id": IdentityId::from_bytes([56; 32]),
            "display_name": "Ada",
            "saved_views": {"watering": "tag:watering"},
        });
        let state =
            LocalState::decode(fields.to_string().as_bytes(), Path::new(FILE_NAME)).unwrap();
        assert_eq!(state.board_name(), "Greenhouse");
        let saved = serde_json::to_value(state).unwrap();
        assert_eq!(saved["version"], 4);
        assert_eq!(saved["boards"][0]["id"], fields["board_id"]);
        assert_eq!(saved["saved_views"], fields["saved_views"]);
    }

    #[test]
    fn rejects_legacy_and_future_versions_before_decoding_ids() {
        let path = Path::new("/pepino/local.json");
        for version in [1, 2] {
            let bytes = serde_json::to_vec(&serde_json::json!({ "version": version })).unwrap();
            assert_eq!(
                LocalState::decode(&bytes, path),
                Err(AppError::IncompatibleLocalState {
                    version,
                    directory: "/pepino".into()
                })
            );
        }
        assert!(matches!(
            LocalState::decode(br#"{"version":4294967296}"#, path),
            Err(AppError::Storage { retryable: false, reason })
                if reason == "unsupported local state version: 4294967296"
        ));
    }
}
