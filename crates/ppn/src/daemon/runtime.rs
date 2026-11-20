use std::{
    io::{self, Write},
    sync::Arc,
    time::{Duration, Instant},
};

use pepino_node::{Node, NodeSettings};
use pepino_sync::SyncEngine;
use tokio::{
    net::UnixListener,
    runtime::Builder,
    signal::unix::{Signal, SignalKind, signal},
    time::timeout,
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

const STARTUP_LIMIT: Duration = Duration::from_secs(8);

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
    let settings = node::settings(config).await?;
    let _lease = match _lease {
        Some(lock) => lock,
        None => lease(&lock).map_err(socket_error)?,
    };
    let path = socket_path(config);
    super::address::validate_path(&path)
        .map_err(|error| CliError::new(ErrorKind::Failure, error.to_string()))?;
    prepare_parent(config, &path).map_err(socket_error)?;
    let socket = Socket::bind(path).await.map_err(socket_error)?;
    loop {
        let outcome = cycle(
            &socket.listener,
            &settings,
            level,
            started,
            &mut terminate,
            &mut interrupt,
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
    level: LogLevel,
    started: Instant,
    terminate: &mut Signal,
    interrupt: &mut Signal,
) -> Result<End, CliError> {
    let node = tokio::select! {
        _ = terminate.recv() => return Ok(End::Stopped),
        _ = interrupt.recv() => return Ok(End::Stopped),
        result = timeout(STARTUP_LIMIT, Node::spawn(settings.clone())) => {
            Arc::new(result.ok().and_then(Result::ok).ok_or_else(|| failure("Could not start the network node."))?)
        }
    };
    let engine = tokio::select! {
        _ = terminate.recv() => None,
        _ = interrupt.recv() => None,
        result = timeout(STARTUP_LIMIT, SyncEngine::start_with_node_settings(node.clone(), settings)) => Some(result),
    };
    let mut engine_stop = Ok(());
    let outcome = match engine {
        Some(Ok(Ok(engine))) => {
            log(level, LogLevel::Info, "daemon ready");
            let end = tokio::select! {
                biased;
                _ = terminate.recv() => Ok(End::Stopped),
                _ = interrupt.recv() => Ok(End::Stopped),
                result = server::serve(listener, &engine, started) => result.map_err(socket_error),
            };
            node.prepare_shutdown();
            engine_stop = match engine.shutdown().await {
                Err(pepino_sync::Error::RestartRequired(_)) if matches!(end, Ok(End::Restart)) => {
                    Ok(())
                }
                result => result.map_err(|error| shutdown_failure("the sync engine", error)),
            };
            end
        }
        Some(_) => Err(failure("Could not start the sync engine.")),
        None => Ok(End::Stopped),
    };
    let node = Arc::try_unwrap(node)
        .map_err(|_| shutdown_failure("the network node", "sync workers still hold it"))?;
    node.shutdown()
        .await
        .map_err(|error| shutdown_failure("the network node", error))?;
    engine_stop.and(outcome)
}

fn shutdown_failure(component: &str, error: impl std::fmt::Display) -> CliError {
    CliError::new(
        ErrorKind::Failure,
        format!("Could not stop {component}. {error}"),
    )
}

fn log(configured: LogLevel, level: LogLevel, message: &str) {
    if configured >= level {
        let _ = writeln!(io::stderr().lock(), "{level:?} {message}");
    }
}

fn socket_error(error: io::Error) -> CliError {
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

fn failure(message: &str) -> CliError {
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
