use std::{
    cell::OnceCell,
    io::{self, Write},
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use pepino_node::{Node, NodeSettings};
use pepino_sync::SyncEngine;
use tokio::{
    net::UnixListener,
    runtime::Builder,
    signal::unix::{Signal, SignalKind, signal},
    sync::Notify,
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
    let mut upgrade_logged = false;
    loop {
        let outcome = cycle(
            &socket.listener,
            &settings,
            async |node| SyncEngine::start_with_node_settings(node, &settings, app.clone()).await,
            level,
            started,
            (&mut terminate, &mut interrupt),
            &mut upgrade_logged,
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
    upgrade_logged: &mut bool,
) -> Result<End, CliError> {
    let startup = Instant::now();
    let engine = OnceCell::new();
    let ready = Notify::new();
    let mut node = None;
    let outcome = {
        let start = async {
            let transport =
                Arc::new(Node::spawn(settings.clone()).await.map_err(|error| {
                    failure(&format!("Could not start the network node. {error}"))
                })?);
            node = Some(transport.clone());
            let sync = start_engine(transport)
                .await
                .map_err(|error| failure(&format!("Could not start the sync engine. {error}")))?;
            let sync = engine.get_or_init(|| sync);
            ready.notify_one();
            log(level, LogLevel::Info, "daemon ready");
            log_catalog_upgrade(sync, level, upgrade_logged).await;
            unreachable!("status monitoring does not end")
        };
        tokio::select! {
            biased;
            _ = terminate.recv() => Ok(End::Stopped),
            _ = interrupt.recv() => Ok(End::Stopped),
            result = server::serve(listener, &engine, &ready, started, startup) => result.map_err(socket_error),
            result = start => result,
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

async fn log_catalog_upgrade(engine: &SyncEngine, level: LogLevel, logged: &mut bool) {
    if !*logged {
        let mut changes = std::pin::pin!(engine.status_changes());
        while let Some(status) = changes.next().await {
            if status.newer_catalog.is_some() {
                log(level, LogLevel::Warn, pepino_app::CATALOG_UPGRADE_NOTICE);
                *logged = true;
                break;
            }
        }
    }
    std::future::pending().await
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
