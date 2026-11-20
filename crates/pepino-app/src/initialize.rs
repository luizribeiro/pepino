use std::{
    fs, io,
    path::{Path, PathBuf},
};

use pepino_core::BoardId;
use pepino_node::{DeviceCredentials, Node, NodeSettings};

use crate::{
    AppError, LocalState, OpenOptions,
    app::{DATABASE, exists, invalid, worker},
    state::{FILE_NAME, InitialNames, path_io_error, storage, sync_directory},
};

pub(super) const PENDING: &str = "local.pending.json";
const STAGING: &str = ".node-init";
const NODE_DATABASE: &str = "node.sqlite";

pub(super) async fn prepare(
    directory: PathBuf,
    credentials_path: PathBuf,
    options: OpenOptions,
) -> Result<LocalState, AppError> {
    let pending = directory.join(PENDING);
    let stage = directory.join(STAGING);
    let staged_database = stage.join(NODE_DATABASE);

    let staged = stage.clone();
    let path = pending.clone();
    let root = directory.clone();
    let (prepared, credentials) = worker("prepare initialization", move || {
        let prepared = if exists(&path)? {
            Prepared::Resume(LocalState::read(&path)?)
        } else {
            let names = InitialNames::try_from(options)?;
            if exists(&root.join(NODE_DATABASE))? {
                return Err(storage(
                    "initialize board",
                    "node.sqlite already exists without local state",
                    false,
                ));
            }
            Prepared::New(names)
        };
        let credentials = credentials(&root, &credentials_path)?;
        match &prepared {
            Prepared::Resume(state) if state.device_id != credentials.device_id() => {
                return Err(storage(
                    "resume initialization",
                    "credentials do not match the pending device ID",
                    false,
                ));
            }
            Prepared::New(_) => {
                remove_staging(&staged)?;
                LocalState::prepare_directory(&staged)?;
            }
            Prepared::Resume(_) => (),
        }
        Ok::<_, AppError>((prepared, credentials))
    })
    .await??;

    let local = match prepared {
        Prepared::Resume(state) => state,
        Prepared::New(names) => {
            let device = credentials.device_id();
            let board = create_board(NodeSettings::offline(credentials, &staged_database)).await?;
            let state = LocalState::new(names, device, board)?;
            worker("record pending board", {
                let state = state.clone();
                let stage = stage.clone();
                move || {
                    sync(&stage)?;
                    state.write(&pending)
                }
            })
            .await??;
            state
        }
    };

    worker("install board database", move || {
        let database = directory.join(NODE_DATABASE);
        if !exists(&database)? {
            fs::rename(&staged_database, &database)
                .map_err(|error| path_io_error(&database, "install board database", error))?;
            sync(&directory)?;
        }
        remove_staging(&stage)
    })
    .await??;
    Ok(local)
}

enum Prepared {
    New(InitialNames),
    Resume(LocalState),
}

fn credentials(directory: &Path, path: &Path) -> Result<DeviceCredentials, AppError> {
    if path.starts_with(directory.join(STAGING))
        || [
            FILE_NAME,
            PENDING,
            DATABASE,
            NODE_DATABASE,
            "node.sqlite-wal",
            "node.sqlite-shm",
            "pepino.sqlite-wal",
            "pepino.sqlite-shm",
            "local.lock",
            "node.lock",
        ]
        .iter()
        .any(|name| path.starts_with(directory.join(name)))
    {
        return Err(invalid(
            "credentials_file",
            "path conflicts with a Pepino data file",
        ));
    }
    if let Some(parent) = path.parent() {
        LocalState::prepare_directory(parent)?;
    }
    DeviceCredentials::load_or_create(path).map_err(|error| match error {
        pepino_node::Error::CredentialIo(error) => {
            path_io_error(path, "load or create credentials", error)
        }
        error => storage("load or create credentials", error, false),
    })
}

async fn create_board(settings: NodeSettings) -> Result<BoardId, AppError> {
    let node = Node::spawn(settings)
        .await
        .map_err(|error| storage("start offline node", error, false))?;
    let board = async {
        let board = node.create_board().await?;
        let id = board.id();
        board.close().await?;
        Ok::<_, pepino_node::Error>(id)
    }
    .await;
    let stopped = node.shutdown().await;
    let id = board.map_err(|error| storage("create encrypted board", error, false))?;
    stopped.map_err(|error| storage("stop offline node", error, false))?;
    Ok(id)
}

pub(super) fn finish(directory: &Path) -> Result<(), AppError> {
    let state = directory.join(FILE_NAME);
    fs::rename(directory.join(PENDING), &state)
        .map_err(|error| path_io_error(&state, "publish local state", error))?;
    sync(directory)
}

fn remove_staging(stage: &Path) -> Result<(), AppError> {
    let remove = || -> io::Result<()> {
        let metadata = match fs::symlink_metadata(stage) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "invalid staging path",
            ));
        }
        for file in [
            NODE_DATABASE,
            "node.sqlite-wal",
            "node.sqlite-shm",
            "node.lock",
        ] {
            match fs::remove_file(stage.join(file)) {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error),
            }
        }
        fs::remove_dir(stage)
    };
    remove().map_err(|error| path_io_error(stage, "remove board staging directory", error))
}

fn sync(directory: &Path) -> Result<(), AppError> {
    sync_directory(directory)
        .map_err(|error| path_io_error(directory, "sync data directory", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
    async fn stages_a_real_encrypted_board_before_publishing_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path();
        let keys = path.join("credentials.cbor");
        let state = prepare(path.to_owned(), keys.clone(), OpenOptions::default())
            .await
            .unwrap();
        assert!(!path.join(FILE_NAME).exists());
        assert_eq!(LocalState::read(&path.join(PENDING)).unwrap(), state);
        let resumed = prepare(path.to_owned(), keys, OpenOptions::default())
            .await
            .unwrap();
        assert_eq!(resumed, state);
        finish(path).unwrap();
        assert_eq!(LocalState::load(path).unwrap(), state);
    }
}
