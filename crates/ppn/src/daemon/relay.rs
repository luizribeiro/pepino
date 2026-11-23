use std::{
    future::pending,
    time::{Duration, Instant},
};

use pepino_core::DeviceId;
use pepino_node::{NodeSettings, Relay, RelayEnd};
use tokio::{net::UnixListener, signal::unix::Signal, time::timeout};

use crate::{
    config::{Config, LogLevel},
    error::CliError,
};

use super::{
    protocol::{DaemonStatus, Operation, Outcome},
    runtime::{STARTUP_LIMIT, failure, log, shutdown_failure, socket_error},
    server::{self, End},
};

fn relay_failure(problem: &str, cause: impl std::fmt::Display) -> CliError {
    failure(&format!("{problem}. {cause}"))
}

const FIRST_RETRY: Duration = Duration::from_millis(250);
const LONGEST_RETRY: Duration = Duration::from_secs(60);
const STABLE_AFTER: Duration = Duration::from_secs(60);

enum Cycle {
    Stopped,
    Restart,
    Failed(String),
}

struct Backoff(Duration);

impl Default for Backoff {
    fn default() -> Self {
        Self(FIRST_RETRY)
    }
}

impl Backoff {
    fn next(&mut self, ran_for: Duration) -> Duration {
        if ran_for >= STABLE_AFTER {
            *self = Self::default();
        }
        let delay = self.0;
        self.0 = (delay * 2).min(LONGEST_RETRY);
        delay
    }
}

pub(super) async fn run(
    listener: &UnixListener,
    settings: &NodeSettings,
    config: &Config,
    level: LogLevel,
    started: Instant,
    terminate: &mut Signal,
    interrupt: &mut Signal,
) -> Result<(), CliError> {
    let mut backoff = Backoff::default();
    loop {
        let began = Instant::now();
        let delay = match cycle(
            listener, settings, config, level, started, terminate, interrupt,
        )
        .await?
        {
            Cycle::Stopped => return Ok(()),
            Cycle::Restart => FIRST_RETRY,
            Cycle::Failed(problem) => {
                log(level, LogLevel::Error, &problem);
                backoff.next(began.elapsed())
            }
        };
        log(level, LogLevel::Warn, "restarting the relay node");
        tokio::select! {
            _ = terminate.recv() => return Ok(()),
            _ = interrupt.recv() => return Ok(()),
            _ = tokio::time::sleep(delay) => (),
        }
    }
}

async fn cycle(
    listener: &UnixListener,
    settings: &NodeSettings,
    config: &Config,
    level: LogLevel,
    started: Instant,
    terminate: &mut Signal,
    interrupt: &mut Signal,
) -> Result<Cycle, CliError> {
    let trusted = config
        .trusted_devices
        .value
        .iter()
        .map(|id| DeviceId::from_bytes(*id.as_bytes()))
        .collect();
    tokio::select! {
        _ = terminate.recv() => return Ok(Cycle::Stopped),
        _ = interrupt.recv() => return Ok(Cycle::Stopped),
        purged = Relay::purge_removed(settings) => {
            purged.map_err(|error| relay_failure("Could not delete removed boards from the relay database", error))?;
        }
    }
    let mut relay = tokio::select! {
        _ = terminate.recv() => return Ok(Cycle::Stopped),
        _ = interrupt.recv() => return Ok(Cycle::Stopped),
        result = timeout(STARTUP_LIMIT, Relay::start(settings.clone(), trusted, config.max_boards.value)) => {
            const PROBLEM: &str = "Could not start the relay node";
            match result {
                Ok(relay) => relay.map_err(|error| relay_failure(PROBLEM, error))?,
                Err(_) => {
                    let limit = format!("startup took longer than {} seconds", STARTUP_LIMIT.as_secs());
                    return Err(relay_failure(PROBLEM, limit));
                }
            }
        }
    };
    let monitor = relay
        .monitor()
        .map_err(|error| relay_failure("Could not monitor the relay node", error))?;
    log(level, LogLevel::Info, "relay ready");
    let end = tokio::select! {
        biased;
        _ = terminate.recv() => Ok(Cycle::Stopped),
        _ = interrupt.recv() => Ok(Cycle::Stopped),
        result = server::serve_with(listener, started, pending::<()>(), async |operation, started| match operation {
            Operation::Status => match monitor.snapshot().await {
                Ok(relay) => Outcome::Status {
                    status: DaemonStatus {
                        pid: std::process::id(),
                        uptime_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                        network: Default::default(),
                        peer_syncs: Default::default(),
                        relay: Some(relay),
                    }
                },
                Err(error) => Outcome::error("failure", &format!("Could not read the relay status. {error}")),
            },
            Operation::RelayDrop { boards } => match monitor.drop_boards(boards).await {
                Ok(dropped) if dropped.boards == 0 => Outcome::error("not_found", "No carried board matches."),
                Ok(dropped) => Outcome::RelayDropped { dropped },
                Err(error) => Outcome::error("failure", &format!("Could not drop the boards. {error}")),
            },
            Operation::Stop => Outcome::Stopping,
            _ => Outcome::error("permission_denied", "This daemon runs in relay mode. Use a device data directory for device operations."),
        }) => match result {
            Ok(End::Stopped) => Ok(Cycle::Stopped),
            Ok(End::Restart) => Ok(Cycle::Restart),
            Err(error) => Err(socket_error(error)),
        },
        result = relay.run() => Ok(match result {
            Ok(RelayEnd::Restart) => Cycle::Restart,
            Ok(RelayEnd::StreamEnded) => Cycle::Failed("a board subscription ended".into()),
            Err(error) => Cycle::Failed(format!("the relay node failed: {error}")),
        }),
    };
    drop(monitor);
    match relay.shutdown().await {
        Err(
            error @ (pepino_node::Error::ShutdownWriterTimeout
            | pepino_node::Error::ShutdownSubscriptionsTimeout),
        ) if matches!(end, Ok(Cycle::Restart | Cycle::Failed(_))) => {
            log(
                level,
                LogLevel::Warn,
                &format!("the relay node closed: {error}"),
            );
        }
        result => result.map_err(|error| shutdown_failure("the relay node", error))?,
    }
    end
}

#[cfg(test)]
#[test]
fn repeated_failures_wait_longer_until_the_relay_runs_stably() {
    let mut backoff = Backoff::default();
    let crashed_after = Duration::from_secs(2);
    let delays: Vec<_> = (0..10).map(|_| backoff.next(crashed_after)).collect();
    assert_eq!(delays[..3], [250, 500, 1000].map(Duration::from_millis));
    assert_eq!(delays[8..], [LONGEST_RETRY, LONGEST_RETRY]);
    assert_eq!(backoff.next(STABLE_AFTER), FIRST_RETRY);
    assert_eq!(backoff.next(crashed_after), FIRST_RETRY * 2);
}

#[cfg(test)]
#[test]
fn a_relay_failure_names_its_cause() {
    let error = relay_failure(
        "Could not start the relay node",
        pepino_node::Error::Storage("unable to open database file".into()),
    );
    assert_eq!(error.kind(), crate::error::ErrorKind::Failure);
    assert_eq!(
        error.to_string(),
        "Could not start the relay node. node storage: unable to open database file"
    );
}
