use std::fs::{self, File, OpenOptions as FileOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pepino_core::{Clock, Hlc, TaskId};
use pepino_store::{DatabaseErrorKind, Store, StoreError};
use tokio::time::{Instant, sleep};

use crate::state::{io_error, path_io_error, storage};
use crate::{
    AppError, ConflictReference, LocalState, MissingReference, OpenOptions, Request, Response,
    WriteRequest,
};

pub(super) const DATABASE: &str = "pepino.sqlite";

pub struct App {
    pub(super) store: Store,
    pub(super) local: LocalState,
    pub(super) directory: PathBuf,
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
                missing_pair(&directory, initialize)?;
            }
            Ok::<_, AppError>(directory)
        })
        .await??;
        let _guard = lock(&directory, lock_timeout).await?;
        let state_dir = directory.clone();
        let (local, fresh) = worker("open local state", move || {
            let fresh = missing_pair(&state_dir, initialize)?;
            let local = if fresh {
                LocalState::load_or_create(&state_dir, options)?
            } else {
                LocalState::load(&state_dir)?
            };
            Ok::<_, AppError>((local, fresh))
        })
        .await??;
        let database = directory.join(DATABASE);
        let runtime = tokio::runtime::Handle::current();
        // Store startup borrows its migration connection through a non-Send future.
        let store = worker("open store", move || {
            if fresh {
                create_database_file(&database)?;
            }
            runtime
                .block_on(Store::open_existing(&database))
                .map_err(|error| store_error_at(&database, error))
        })
        .await??;
        store
            .bind_local(local.board_id, local.identity_id, local.device_id)
            .await
            .map_err(|error| store_error_at(&directory.join(DATABASE), error))?;
        let state_dir = directory.clone();
        let local = worker("migrate local state", move || local.migrate(&state_dir)).await??;
        let hlc = store
            .latest_hlc(local.board_id)
            .await
            .map_err(store_error)?
            .unwrap_or(Hlc {
                wall_ms: now,
                counter: 0,
            });
        Ok(Self {
            clock: Clock::from_hlc(hlc),
            store,
            local,
            directory,
            lock_timeout,
        })
    }

    pub fn local_state(&self) -> &LocalState {
        &self.local
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
                        .task_titles(self.local.board_id, &[id])
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
    match (exists(&state)?, exists(&database)?) {
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

async fn lock(directory: &Path, timeout: Duration) -> Result<File, AppError> {
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

fn store_error_at(path: &Path, error: StoreError) -> AppError {
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
