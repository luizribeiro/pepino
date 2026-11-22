use std::{
    cell::OnceCell,
    collections::BTreeSet,
    io::{self, Write},
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use pepino_core::BoardId;
use pepino_node::{Node, NodeSettings};
use pepino_sync::SyncEngine;
use tokio::{
    net::UnixListener,
    runtime::Builder,
    signal::unix::{Signal, SignalKind, signal},
    sync::{Notify, oneshot},
};

use crate::{
    config::{Config, LogLevel},
    error::{CliError, ErrorKind},
};

use super::{
    address::{prepare_parent, socket_path},
    node,
    server::{self, End},
    socket::{Socket, lease},
};

#[cfg(all(test, pepino_network_tests))]
mod startup_tests;

pub(super) const STARTUP_LIMIT: Duration = Duration::from_secs(8);
const STARTUP_WARNING: Duration = Duration::from_secs(45);

pub fn run(config: &Config, level: LogLevel) -> Result<(), CliError> {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| failure("Could not start the daemon runtime."))?;
    nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o077));
    let result = runtime.block_on(run_async(config, level));
    runtime.shutdown_timeout(Duration::from_secs(1));
    result
}

async fn run_async(config: &Config, level: LogLevel) -> Result<(), CliError> {
    let started = Instant::now();
    let mut terminate =
        signal(SignalKind::terminate()).map_err(|_| failure("Could not listen for SIGTERM."))?;
    let mut interrupt =
        signal(SignalKind::interrupt()).map_err(|_| failure("Could not listen for SIGINT."))?;
    let directory = config.data_dir.value.as_path();
    let lock = directory.join("daemon.lock");
    let _lease = if directory.join("local.lock").exists() {
        Some(lease(&lock).map_err(socket_error)?)
    } else {
        None
    };
    let relay = pepino_app::is_relay(directory)?;
    let (settings, app) = if relay {
        (node::relay_settings(config)?, None)
    } else {
        let (settings, app) = tokio::select! {
            _ = terminate.recv() => return Ok(()),
            _ = interrupt.recv() => return Ok(()),
            prepared = node::settings(config) => prepared?,
        };
        (settings, Some(Arc::new(app)))
    };
    let _lease = match _lease {
        Some(lock) => lock,
        None => lease(&lock).map_err(socket_error)?,
    };
    let path = socket_path(config);
    super::address::validate_path(&path)
        .map_err(|error| CliError::new(ErrorKind::Failure, error.to_string()))?;
    prepare_parent(config, &path).map_err(socket_error)?;
    let socket = Socket::bind(path).await.map_err(socket_error)?;
    let Some(app) = app else {
        super::relay::run(
            &socket.listener,
            &settings,
            config,
            level,
            started,
            &mut terminate,
            &mut interrupt,
        )
        .await?;
        log(level, LogLevel::Info, "daemon stopped");
        return Ok(());
    };
    let mut logged = StatusLog::default();
    loop {
        let outcome = cycle(
            &socket.listener,
            &settings,
            async |node| SyncEngine::start_with_node_settings(node, &settings, app.clone()).await,
            level,
            started,
            (&mut terminate, &mut interrupt),
            &mut logged,
        )
        .await?;
        if matches!(outcome, End::Stopped) {
            log(level, LogLevel::Info, "daemon stopped");
            return Ok(());
        }
        log(
            level,
            LogLevel::Warn,
            "sync worker failed; restarting the engine and node",
        );
        tokio::select! {
            _ = terminate.recv() => return Ok(()),
            _ = interrupt.recv() => return Ok(()),
            _ = tokio::time::sleep(Duration::from_millis(250)) => (),
        }
    }
}

async fn cycle(
    listener: &UnixListener,
    settings: &NodeSettings,
    start_engine: impl AsyncFnOnce(Arc<Node>) -> Result<SyncEngine, pepino_sync::Error>,
    level: LogLevel,
    started: Instant,
    (terminate, interrupt): (&mut Signal, &mut Signal),
    logged: &mut StatusLog,
) -> Result<End, CliError> {
    let startup = Instant::now();
    let engine = OnceCell::new();
    let ready = Notify::new();
    let (stop_start, stopping) = oneshot::channel();
    let mut node = None;
    let outcome = {
        let start = async {
            let transport = Node::spawn_until_stopped(settings.clone(), async {
                let _ = stopping.await;
            })
            .await
            .map_err(|error| failure(&format!("Could not start the network node. {error}")))?;
            let Some(transport) = transport else {
                return Ok(());
            };
            let transport = Arc::new(transport);
            node = Some(transport.clone());
            let sync = start_engine(transport)
                .await
                .map_err(|error| failure(&format!("Could not start the sync engine. {error}")))?;
            engine.get_or_init(|| sync);
            ready.notify_one();
            Ok::<_, CliError>(())
        };
        let control = async {
            tokio::select! {
                biased;
                _ = terminate.recv() => Ok(End::Stopped),
                _ = interrupt.recv() => Ok(End::Stopped),
                result = server::serve(listener, &engine, &ready, started, startup) => result.map_err(socket_error),
                () = startup_warning(STARTUP_WARNING, || {
                    if engine.get().is_none() {
                        let message = format!(
                            "daemon still starting after {} seconds",
                            STARTUP_WARNING.as_secs()
                        );
                        log(level, LogLevel::Warn, &message);
                    }
                }) => unreachable!("startup warning does not end control service"),
            }
        };
        tokio::pin!(start, control);
        tokio::select! {
            biased;
            outcome = &mut control => {
                let _ = stop_start.send(());
                // Once the node is live, the engine must hand back its workers for shutdown.
                start.await.and(outcome)
            }
            result = &mut start => match result {
                Err(error) => Err(error),
                Ok(()) => {
                    let Some(sync) = engine.get() else {
                        return Ok(End::Stopped);
                    };
                    log(level, LogLevel::Info, "daemon ready");
                    tokio::select! {
                        biased;
                        outcome = &mut control => outcome,
                        () = log_status(sync, level, logged) => unreachable!("status monitoring does not end"),
                    }
                }
            }
        }
    };
    if let Some(node) = node {
        let mut engine_stop = Ok(());
        if let Some(engine) = engine.into_inner() {
            node.prepare_shutdown();
            engine_stop = match engine.shutdown().await {
                Err(pepino_sync::Error::RestartRequired(_))
                    if matches!(outcome, Ok(End::Restart)) =>
                {
                    Ok(())
                }
                result => result.map_err(|error| shutdown_failure("the sync engine", error)),
            };
        }
        let node = Arc::try_unwrap(node)
            .map_err(|_| shutdown_failure("the network node", "sync workers still hold it"))?;
        node.shutdown()
            .await
            .map_err(|error| shutdown_failure("the network node", error))?;
        engine_stop?;
    }
    outcome
}

async fn startup_warning(delay: Duration, log: impl FnOnce()) {
    tokio::time::sleep(delay).await;
    log();
    std::future::pending().await
}

#[cfg(test)]
#[tokio::test]
async fn the_startup_warning_fires_once_without_ending_startup() {
    let (warned, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let startup = startup_warning(Duration::ZERO, || warned.send(()).unwrap());
    tokio::pin!(startup);
    tokio::select! {
        result = observed.recv() => assert_eq!(result, Some(())),
        () = &mut startup => panic!("startup was interrupted by its warning"),
    }
    assert!(futures_util::poll!(&mut startup).is_pending());
    assert!(observed.try_recv().is_err());
}

#[derive(Default)]
struct StatusLog {
    upgrade_logged: bool,
    reported: BTreeSet<(BoardId, String)>,
}

impl StatusLog {
    /// Lines for board conditions not yet reported. `opened` holds the boards seen since
    /// this engine started; an engine that is restarting reports no closures.
    fn board_lines(
        &mut self,
        opened: &mut BTreeSet<BoardId>,
        node: &pepino_node::NodeStatus,
        restarting: bool,
    ) -> Vec<(LogLevel, String)> {
        let current: Vec<_> = node
            .boards
            .iter()
            .flat_map(|(id, board)| {
                let near_limit = board
                    .near_header_limit()
                    .then(|| super::protocol::NEAR_MEMBER_LIMIT_NOTICE.to_owned());
                let unmeasured = board
                    .membership_header_error
                    .as_ref()
                    .map(|error| format!("could not measure membership headers: {error}"));
                [
                    (LogLevel::Warn, near_limit),
                    (LogLevel::Warn, board.operation_read_error.clone()),
                    (LogLevel::Debug, unmeasured),
                ]
                .into_iter()
                .filter_map(move |(level, message)| Some((*id, level, message?)))
            })
            .collect();
        opened.extend(node.boards.keys());
        if !restarting {
            self.reported.retain(|(id, message)| {
                !opened.contains(id)
                    || current
                        .iter()
                        .any(|(board, _, text)| (board, text) == (id, message))
            });
            opened.retain(|id| node.boards.contains_key(id));
        }
        current
            .into_iter()
            .filter(|(id, _, message)| self.reported.insert((*id, message.clone())))
            .map(|(id, level, message)| (level, board_line(id, &message)))
            .collect()
    }
}

// A board id is a capability, so log lines carry only its short form.
fn board_line(id: BoardId, message: &str) -> String {
    crate::error::shorten_message_ids(&format!("board {id}: {message}"))
}

async fn log_status(engine: &SyncEngine, level: LogLevel, logged: &mut StatusLog) {
    let mut opened = BTreeSet::new();
    let mut changes = std::pin::pin!(engine.status_changes());
    while let Some(status) = changes.next().await {
        if !logged.upgrade_logged && status.newer_catalog.is_some() {
            log(level, LogLevel::Warn, pepino_app::CATALOG_UPGRADE_NOTICE);
            logged.upgrade_logged = true;
        }
        let restarting = status.restart_required.is_some();
        for (severity, line) in logged.board_lines(&mut opened, &status.node, restarting) {
            log(level, severity, &line);
        }
    }
    std::future::pending().await
}

#[cfg(test)]
mod status_log_tests {
    use super::*;
    use pepino_node::{BoardStatus, HEADER_ITEM_LIMIT, HEADER_ITEM_WARNING, NodeStatus};

    const HOUSE: BoardId = BoardId::from_bytes([65; 32]);
    const ALLOTMENT: BoardId = BoardId::from_bytes([66; 32]);
    const UNREADABLE: &str = "read a board operation during sync: failed decoding incoming header bytes: could not decode header from cbor: LengthTooLarge";

    fn node(boards: &[(BoardId, BoardStatus)]) -> NodeStatus {
        NodeStatus {
            boards: boards.iter().cloned().collect(),
            ..Default::default()
        }
    }

    fn measured(largest_membership_header_item: usize) -> BoardStatus {
        BoardStatus {
            largest_membership_header_item,
            ..Default::default()
        }
    }

    fn near_limit_line(board: BoardId) -> (LogLevel, String) {
        let notice = crate::daemon::protocol::NEAR_MEMBER_LIMIT_NOTICE;
        let short = &board.to_string()[..6];
        (LogLevel::Warn, format!("board {short}: {notice}"))
    }

    #[test]
    fn board_lines_never_carry_the_full_board_id() {
        let mut log = StatusLog::default();
        let board = BoardStatus {
            operation_read_error: Some(UNREADABLE.into()),
            membership_header_error: Some(format!("node storage: no log for {HOUSE}")),
            ..measured(HEADER_ITEM_LIMIT)
        };
        let lines = log.board_lines(&mut BTreeSet::new(), &node(&[(HOUSE, board)]), false);
        let short = &HOUSE.to_string()[..6];
        assert_eq!(
            lines,
            [
                near_limit_line(HOUSE),
                (LogLevel::Warn, format!("board {short}: {UNREADABLE}")),
                (
                    LogLevel::Debug,
                    format!(
                        "board {short}: could not measure membership headers: node storage: no log for {short}"
                    )
                ),
            ]
        );
        assert!(
            lines
                .iter()
                .all(|(_, line)| !line.contains(&HOUSE.to_string()))
        );
    }

    #[test]
    fn a_board_near_the_member_limit_is_reported_once() {
        let mut log = StatusLog::default();
        let mut opened = BTreeSet::new();
        let small = node(&[(HOUSE, measured(547)), (ALLOTMENT, measured(1_236))]);
        assert!(log.board_lines(&mut opened, &small, false).is_empty());
        let grown = node(&[
            (HOUSE, measured(HEADER_ITEM_WARNING + 1)),
            (ALLOTMENT, measured(1_236)),
        ]);
        assert_eq!(
            log.board_lines(&mut opened, &grown, false),
            [near_limit_line(HOUSE)]
        );
        let both = node(&[
            (HOUSE, measured(HEADER_ITEM_LIMIT)),
            (ALLOTMENT, measured(14_000)),
        ]);
        assert_eq!(
            log.board_lines(&mut opened, &both, false),
            [near_limit_line(ALLOTMENT)]
        );
        assert!(log.board_lines(&mut opened, &both, false).is_empty());
    }

    #[test]
    fn reports_survive_an_engine_restart_and_end_when_the_board_closes() {
        let mut log = StatusLog::default();
        let near = node(&[(HOUSE, measured(HEADER_ITEM_WARNING + 1))]);
        let mut opened = BTreeSet::new();
        assert_eq!(
            log.board_lines(&mut opened, &near, false),
            [near_limit_line(HOUSE)]
        );
        assert!(log.board_lines(&mut opened, &node(&[]), true).is_empty());

        let mut reopened = BTreeSet::new();
        assert!(log.board_lines(&mut reopened, &node(&[]), false).is_empty());
        assert!(log.board_lines(&mut reopened, &near, false).is_empty());

        assert!(log.board_lines(&mut reopened, &node(&[]), false).is_empty());
        assert_eq!(
            log.board_lines(&mut reopened, &near, false),
            [near_limit_line(HOUSE)]
        );
    }

    #[test]
    fn a_read_error_is_reported_again_after_it_cleared() {
        let mut log = StatusLog::default();
        let mut opened = BTreeSet::new();
        let failing = node(&[(
            HOUSE,
            BoardStatus {
                operation_read_error: Some(UNREADABLE.into()),
                ..Default::default()
            },
        )]);
        let clean = node(&[(HOUSE, BoardStatus::default())]);
        assert_eq!(log.board_lines(&mut opened, &failing, false).len(), 1);
        assert!(log.board_lines(&mut opened, &failing, false).is_empty());
        assert!(log.board_lines(&mut opened, &clean, false).is_empty());
        assert_eq!(log.board_lines(&mut opened, &failing, false).len(), 1);
    }
}

pub(super) fn shutdown_failure(component: &str, error: impl std::fmt::Display) -> CliError {
    CliError::new(
        ErrorKind::Failure,
        format!("Could not stop {component}. {error}"),
    )
}

pub(super) fn log(configured: LogLevel, level: LogLevel, message: &str) {
    if configured >= level {
        let _ = writeln!(io::stderr().lock(), "{level:?} {message}");
    }
}

pub(super) fn socket_error(error: io::Error) -> CliError {
    let (kind, message) = match error.kind() {
        io::ErrorKind::AddrInUse => (
            ErrorKind::Busy,
            "A daemon is already running for this data directory or socket.",
        ),
        io::ErrorKind::PermissionDenied => (
            ErrorKind::PermissionDenied,
            "Permission denied opening the daemon lock or control socket.",
        ),
        _ => (
            ErrorKind::Failure,
            "Could not open or serve the daemon control socket.",
        ),
    };
    CliError::new(kind, message)
}

pub(super) fn failure(message: &str) -> CliError {
    CliError::new(ErrorKind::Failure, message)
}

#[cfg(test)]
#[tokio::test]
async fn an_uninitialized_daemon_creates_no_runtime_state() {
    use crate::config::{Platform, PlatformDirs};
    let home = tempfile::tempdir().unwrap();
    let dirs = PlatformDirs::new(Platform::Linux, home.path(), None, None);
    let config = Config::defaults(&dirs).unwrap();
    assert_eq!(
        run_async(&config, LogLevel::Off).await.unwrap_err().kind(),
        ErrorKind::NotInitialized
    );
    assert!(!config.data_dir.value.as_path().exists());
}

#[cfg(test)]
#[test]
fn shutdown_diagnostic_names_the_failed_component_and_stage() {
    let error = shutdown_failure(
        "the network node",
        pepino_node::Error::Worker("timed out closing board subscriptions".into()),
    );
    assert_eq!(error.kind(), ErrorKind::Failure);
    assert_eq!(
        error.to_string(),
        "Could not stop the network node. node runtime: timed out closing board subscriptions"
    );
}
