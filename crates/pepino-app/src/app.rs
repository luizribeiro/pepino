use std::fs::{self, File, OpenOptions as FileOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pepino_core::{BoardId, Clock, DeviceId, EventId, Hlc, TaskId};
use pepino_store::{AppendOutcome, DatabaseErrorKind, Store, StoreError};
use tokio::time::{Instant, sleep};

use crate::state::{io_error, path_io_error, storage};
use crate::{
    AppError, ConflictReference, LocalState, MissingReference, OpenOptions, Request, Response,
    WriteRequest,
};

pub(super) const DATABASE: &str = "pepino.sqlite";

#[derive(Debug, thiserror::Error)]
pub enum SyncStoreError {
    #[error(transparent)]
    Lock(#[from] AppError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub struct App {
    pub(super) store: Store,
    pub(super) local: LocalState,
    pub(super) directory: PathBuf,
    pub(super) credentials_file: PathBuf,
    lock_timeout: Duration,
    pub(super) clock: Clock,
}

impl App {
    pub async fn init(data_dir: impl AsRef<Path>, now: u64) -> Result<Self, AppError> {
        Self::init_with_options(data_dir, now, OpenOptions::default()).await
    }

    pub async fn init_with_options(
        data_dir: impl AsRef<Path>,
        now: u64,
        options: OpenOptions,
    ) -> Result<Self, AppError> {
        Self::startup(data_dir, now, options, true).await
    }

    pub async fn open(data_dir: impl AsRef<Path>, now: u64) -> Result<Self, AppError> {
        Self::open_with_options(data_dir, now, OpenOptions::default()).await
    }

    pub async fn open_with_options(
        data_dir: impl AsRef<Path>,
        now: u64,
        options: OpenOptions,
    ) -> Result<Self, AppError> {
        Self::startup(data_dir, now, options, false).await
    }

    async fn startup(
        data_dir: impl AsRef<Path>,
        now: u64,
        options: OpenOptions,
        initialize: bool,
    ) -> Result<Self, AppError> {
        let directory = data_dir.as_ref().to_owned();
        let lock_timeout = options.lock_timeout;
        let directory = worker("locate local state", move || {
            let directory = std::path::absolute(directory)
                .map_err(|error| io_error("resolve data directory", error))?;
            if initialize {
                LocalState::prepare_directory(&directory)?;
            }
            if !exists(&directory.join("local.lock"))? {
                if exists(&directory.join(crate::state::FILE_NAME))? {
                    LocalState::load(&directory)?;
                }
                missing_pair(&directory, initialize)?;
            }
            Ok::<_, AppError>(directory)
        })
        .await??;
        let _guard = lock(&directory, lock_timeout).await?;
        let credentials_file = options
            .credentials_file
            .as_ref()
            .map(|path| {
                std::path::absolute(path)
                    .map_err(|error| io_error("resolve credentials file", error))
            })
            .transpose()?
            .unwrap_or_else(|| directory.join("credentials.cbor"));
        let state_dir = directory.clone();
        let existing = worker("open local state", move || {
            if exists(&state_dir.join(crate::state::FILE_NAME))? {
                let local = LocalState::load_migrating(&state_dir)?;
                missing_pair(&state_dir, initialize)?;
                Ok::<_, AppError>(Some(local))
            } else {
                missing_pair(&state_dir, initialize)?;
                Ok(None)
            }
        })
        .await??;
        let fresh = existing.is_none();
        let local = match existing {
            Some(local) => local,
            None => {
                crate::initialize::prepare(directory.clone(), credentials_file.clone(), options)
                    .await?
            }
        };
        let database = directory.join(DATABASE);
        let store = open_store(database, fresh).await?;
        store
            .bind_local(local.board_id(), local.identity_id, local.device_id)
            .await
            .map_err(|error| store_error_at(&directory.join(DATABASE), error))?;
        let hlc = store
            .latest_hlc(local.board_id())
            .await
            .map_err(store_error)?
            .unwrap_or(Hlc {
                wall_ms: now,
                counter: 0,
            });
        if fresh {
            let state_dir = directory.clone();
            worker("publish local state", move || {
                crate::initialize::finish(&state_dir)
            })
            .await??;
        }
        Ok(Self {
            clock: Clock::from_hlc(hlc),
            store,
            local,
            directory,
            credentials_file,
            lock_timeout,
        })
    }

    pub fn local_state(&self) -> &LocalState {
        &self.local
    }

    /// Access the event store for reads.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Append a transport delivery under the same directory lock used by local requests.
    pub async fn append_received(
        &self,
        board: BoardId,
        bytes: &[u8],
        signer: DeviceId,
    ) -> Result<AppendOutcome, SyncStoreError> {
        let _guard = lock(&self.directory, self.lock_timeout).await?;
        Ok(self.store.append(board, bytes, Some(signer)).await?)
    }

    /// Retain an undecodable delivery under the local request lock.
    pub async fn record_undecodable(
        &self,
        board: BoardId,
        signer: DeviceId,
        bytes: &[u8],
    ) -> Result<(), SyncStoreError> {
        let _guard = lock(&self.directory, self.lock_timeout).await?;
        Ok(self.store.record_undecodable(board, signer, bytes).await?)
    }

    /// Persist a publication receipt under the directory lock after publishing.
    pub async fn record_publication(
        &self,
        board: BoardId,
        event: EventId,
        operation: EventId,
    ) -> Result<(), SyncStoreError> {
        self.record_publication_for(board, event, operation, None)
            .await
    }

    /// Persist a publication receipt for the current node database under the directory lock.
    pub async fn record_publication_for(
        &self,
        board: BoardId,
        event: EventId,
        operation: EventId,
        node_instance: Option<[u8; 16]>,
    ) -> Result<(), SyncStoreError> {
        let _guard = lock(&self.directory, self.lock_timeout).await?;
        Ok(self
            .store
            .mark_published_for(board, event, operation, node_instance)
            .await?)
    }

    pub async fn handle(&mut self, request: Request, now: u64) -> Result<Response, AppError> {
        let guard = lock(&self.directory, self.lock_timeout).await?;
        let deleted_conflicts = matches!(
            &request,
            Request::Write(WriteRequest::AddTask { .. } | WriteRequest::Link { .. })
        );
        let result = match request {
            Request::Read(request) => self.read(request).await,
            Request::Write(WriteRequest::SaveView { name, filter }) => {
                self.save_view(name, &filter, guard).await
            }
            Request::Write(WriteRequest::DeleteView { name, force }) => {
                self.delete_view(name, force, guard).await
            }
            Request::Write(WriteRequest::ApplyToTasks { tasks, change }) => {
                self.apply_to_tasks(tasks, change, now).await
            }
            Request::Write(request) => {
                let board = self.board().await?;
                let mut batch = self.batch(now).await?;
                match self.plan_write(&board, &mut batch, request) {
                    Ok(()) => self.commit_batch(&board, batch).await,
                    Err(error) => Err(error),
                }
            }
        };
        match result {
            Err(AppError::Conflict {
                field,
                reference: ConflictReference::Task { task, .. },
            }) => {
                let title = if let Ok(id) = task.parse::<TaskId>() {
                    self.store
                        .task_titles(self.local.board_id(), &[id])
                        .await
                        .ok()
                        .and_then(|mut titles| titles.remove(&id))
                } else {
                    None
                };
                Err(if deleted_conflicts {
                    AppError::Conflict {
                        field,
                        reference: ConflictReference::Task { task, title },
                    }
                } else {
                    AppError::NotFound {
                        field,
                        reference: MissingReference::DeletedTask { task, title },
                    }
                })
            }
            result => result,
        }
    }
}

pub(super) fn exists(path: &Path) -> Result<bool, AppError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(path_io_error(path, "inspect path", error)),
    }
}

fn missing_pair(directory: &Path, initialize: bool) -> Result<bool, AppError> {
    let state = directory.join(crate::state::FILE_NAME);
    let database = directory.join(DATABASE);
    let state_exists = exists(&state)?;
    if !state_exists && exists(&directory.join(crate::initialize::PENDING))? {
        return if initialize {
            Ok(true)
        } else {
            Err(AppError::NotInitialized {
                directory: directory.display().to_string(),
            })
        };
    }
    match (state_exists, exists(&database)?) {
        (true, true) => Ok(false),
        (false, false) if initialize => Ok(true),
        (false, false) => Err(AppError::NotInitialized {
            directory: directory.display().to_string(),
        }),
        (false, true) => Err(storage(
            "local state mismatch",
            format_args!("{} exists without {}", database.display(), state.display()),
            false,
        )),
        (true, false) => Err(storage(
            "local state mismatch",
            format_args!("{} exists without {}", state.display(), database.display()),
            false,
        )),
    }
}

pub(super) async fn open_store(database: PathBuf, fresh: bool) -> Result<Store, AppError> {
    let runtime = tokio::runtime::Handle::current();
    // Store startup borrows its migration connection through a non-Send future.
    worker("open store", move || {
        if fresh {
            create_database_file(&database)?;
        }
        runtime
            .block_on(Store::open_existing(&database))
            .map_err(|error| store_error_at(&database, error))
    })
    .await?
}

fn create_database_file(path: &Path) -> Result<(), AppError> {
    let mut options = FileOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(path_io_error(path, "create database", error)),
    }
}

pub(super) async fn worker<T: Send + 'static>(
    context: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| storage(context, error, false))
}

pub(super) async fn lock(directory: &Path, timeout: Duration) -> Result<File, AppError> {
    let path = directory.join("local.lock");
    let mut file = worker("open lock file", move || {
        let mut options = FileOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(&path)
            .map_err(|error| path_io_error(&path, "open data directory lock", error))
    })
    .await??;

    let started = Instant::now();
    let mut delay = Duration::from_millis(1);
    loop {
        let (returned, result) = worker("try data directory lock", move || {
            let result = file.try_lock();
            (file, result)
        })
        .await?;
        file = returned;
        match result {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => {
                return Err(path_io_error(
                    &directory.join("local.lock"),
                    "lock local state",
                    error,
                ));
            }
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(AppError::Busy);
        }
        sleep(delay.min(remaining)).await;
        if started.elapsed() >= timeout {
            return Err(AppError::Busy);
        }
        delay = (delay * 2).min(Duration::from_millis(50));
    }
}

pub(super) fn store_error(error: StoreError) -> AppError {
    store_error_at(Path::new(DATABASE), error)
}

pub(super) fn store_error_at(path: &Path, error: StoreError) -> AppError {
    match error {
        StoreError::Database {
            kind: DatabaseErrorKind::PermissionDenied,
            source,
        }
        | StoreError::Migration {
            kind: DatabaseErrorKind::PermissionDenied,
            source,
        } => AppError::PermissionDenied {
            path: path.display().to_string(),
            reason: source.to_string(),
        },
        StoreError::Database { kind, source } => storage(
            "database operation",
            source,
            kind == DatabaseErrorKind::Busy,
        ),
        StoreError::Migration { kind, source } => storage(
            "database migration",
            source,
            kind == DatabaseErrorKind::Busy,
        ),
        error => storage("pepino store", error, false),
    }
}

pub(super) fn invalid(field: impl Into<String>, reason: impl std::fmt::Display) -> AppError {
    AppError::InvalidInput {
        field: field.into(),
        reason: reason.to_string(),
        diagnostic: None,
    }
}
