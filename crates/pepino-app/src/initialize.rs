use std::{
    fs, io,
    path::{Path, PathBuf},
};

use pepino_core::BoardId;
use pepino_node::{DeviceCredentials, Node, NodeSettings};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

use crate::{
    AppError, LocalState, OpenOptions,
    app::{DATABASE, exists, invalid, worker},
    state::{FILE_NAME, InitialNames, path_io_error, storage, sync_directory},
};

pub(super) const PENDING: &str = "local.pending.json";
const STAGING: &str = ".node-init";
pub(super) const NODE_DATABASE: &str = "node.sqlite";

pub(super) async fn prepare(
    directory: PathBuf,
    credentials_path: PathBuf,
    options: OpenOptions,
    now: u64,
) -> Result<LocalState, AppError> {
    let pending = directory.join(PENDING);
    let stage = directory.join(STAGING);
    let staged_database = stage.join(NODE_DATABASE);

    let staged = stage.clone();
    let path = pending.clone();
    let root = directory.clone();
    let (prepared, credentials) = worker("prepare initialization", move || {
        let prepared = if exists(&path)? {
            Prepared::Resume(Box::new(LocalState::read(&path)?))
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
        Prepared::Resume(state) => *state,
        Prepared::New(names) => {
            let device = credentials.device_id();
            let board =
                create_board(NodeSettings::offline(credentials.clone(), &staged_database)).await?;
            let mut state = LocalState::new(names, device, board)?;
            state.reconcile_identity(device, &credentials, Some(now))?;
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

    let database = directory.join(NODE_DATABASE);
    let installed = worker("check for installed board database", {
        let database = database.clone();
        move || exists(&database)
    })
    .await??;
    if !installed {
        // The database rename cannot carry committed pages still in the WAL.
        checkpoint_staged_database(&staged_database).await?;
    }
    worker("install board database", move || {
        if !installed {
            fs::rename(&staged_database, &database)
                .map_err(|error| path_io_error(&database, "install board database", error))?;
            sync(&directory)?;
        }
        remove_staging(&stage)
    })
    .await??;
    Ok(local)
}

async fn checkpoint_staged_database(path: &Path) -> Result<(), AppError> {
    let failed = |error| storage("checkpoint initial board database", error, false);
    let options = SqliteConnectOptions::new().filename(path);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .map_err(failed)?;
    let (busy, _, _): (i64, i64, i64) = sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
        .fetch_one(&mut connection)
        .await
        .map_err(failed)?;
    connection.close().await.map_err(failed)?;
    if busy != 0 {
        return Err(storage(
            "checkpoint initial board database",
            "staged node database is still in use",
            true,
        ));
    }
    Ok(())
}

enum Prepared {
    New(InitialNames),
    Resume(Box<LocalState>),
}

pub(super) fn credentials(directory: &Path, path: &Path) -> Result<DeviceCredentials, AppError> {
    if path.starts_with(directory.join(STAGING))
        || [
            FILE_NAME,
            crate::device_request::REQUEST,
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
    use crate::{App, OpenOptions};

    #[tokio::test]
    async fn installing_a_pending_board_preserves_committed_wal_pages() {
        use sqlx::sqlite::SqliteJournalMode;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path();
        let stage = path.join(STAGING);
        fs::create_dir(&stage).unwrap();
        let keys = DeviceCredentials::load_or_create(path.join("credentials.cbor")).unwrap();
        let board = create_board(NodeSettings::offline(
            keys.clone(),
            stage.join(NODE_DATABASE),
        ))
        .await
        .unwrap();
        let names = InitialNames::try_from(OpenOptions {
            display_name: Some("Ada Gardener".into()),
            board_name: Some("Greenhouse".into()),
            ..OpenOptions::default()
        })
        .unwrap();
        let pending = LocalState::new(names, keys.device_id(), board).unwrap();
        pending.write(&path.join(PENDING)).unwrap();

        let options = SqliteConnectOptions::new()
            .filename(stage.join(NODE_DATABASE))
            .journal_mode(SqliteJournalMode::Wal)
            .pragma("wal_autocheckpoint", "0");
        let mut prior = SqliteConnection::connect_with(&options).await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE enrollment_probe (task TEXT NOT NULL);
            INSERT INTO enrollment_probe VALUES ('Catalogue the seed bank');",
        )
        .execute(&mut prior)
        .await
        .unwrap();
        assert!(fs::metadata(stage.join("node.sqlite-wal")).unwrap().len() > 32);

        let app = App::init(path, 1_795_144_800_000).await.unwrap();
        assert_eq!(app.local_state(), &pending);
        let mut installed =
            SqliteConnection::connect_with(&options.filename(path.join(NODE_DATABASE)))
                .await
                .unwrap();
        let retained: String = sqlx::query_scalar("SELECT task FROM enrollment_probe")
            .fetch_one(&mut installed)
            .await
            .unwrap();
        assert_eq!(retained, "Catalogue the seed bank");
        prior.close().await.unwrap();
    }

    #[tokio::test]
    #[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
    async fn resumes_a_pending_board_without_exposing_partial_local_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path();
        let credentials = path.join("credentials.cbor");
        let now = 1_795_144_800_000;
        let abandoned = path.join(STAGING);
        fs::create_dir(&abandoned).unwrap();
        fs::write(abandoned.join(NODE_DATABASE), b"interrupted").unwrap();
        let pending = prepare(
            path.to_owned(),
            credentials.clone(),
            OpenOptions::default(),
            now,
        )
        .await
        .unwrap();
        assert_eq!(LocalState::read(&path.join(PENDING)).unwrap(), pending);
        assert!(matches!(
            App::open(path, now).await,
            Err(AppError::NotInitialized { .. })
        ));
        assert!(!path.join(FILE_NAME).exists());
        assert!(!path.join(DATABASE).exists());
        assert!(!abandoned.exists());

        fs::create_dir(path.join(DATABASE)).unwrap();
        assert!(App::init(path, now).await.is_err());
        assert!(!path.join(FILE_NAME).exists());
        fs::remove_dir(path.join(DATABASE)).unwrap();

        let app = App::init(path, now).await.unwrap();
        assert_eq!(app.local_state(), &pending);
        assert!(!path.join(PENDING).exists());
        let keys = DeviceCredentials::load_or_create(&credentials).unwrap();
        assert_eq!(keys.device_id(), pending.device_id);
        let node = Node::spawn(NodeSettings::offline(keys, path.join(NODE_DATABASE)))
            .await
            .unwrap();
        let board = node.open_board(pending.board_id()).await.unwrap();
        board
            .wait_until_joined(std::time::Duration::from_secs(60))
            .await
            .unwrap();
        board.close().await.unwrap();
        node.shutdown().await.unwrap();
    }
}
