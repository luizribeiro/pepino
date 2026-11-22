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

const FIRST_RETRY: Duration = Duration::from_millis(250);
const LONGEST_RETRY: Duration = Duration::from_secs(60);
const STABLE_AFTER: Duration = Duration::from_secs(60);

enum Cycle {
    Stopped,
    Restart,
    Failed(&'static str),
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
                log(level, LogLevel::Error, problem);
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
            purged.map_err(|_| failure("Could not delete removed boards from the relay database."))?;
        }
    }
    let mut relay = tokio::select! {
        _ = terminate.recv() => return Ok(Cycle::Stopped),
        _ = interrupt.recv() => return Ok(Cycle::Stopped),
        result = timeout(STARTUP_LIMIT, Relay::start(settings.clone(), trusted, config.max_boards.value)) => {
            result.ok().and_then(Result::ok).ok_or_else(|| failure("Could not start the relay node."))?
        }
    };
    let monitor = relay
        .monitor()
        .map_err(|_| failure("Could not monitor the relay node."))?;
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
                        relay: Some(relay),
                    }
                },
                Err(_) => Outcome::error("failure", "Could not read the relay status."),
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
            Ok(RelayEnd::StreamEnded) => Cycle::Failed("a board subscription ended"),
            Err(_) => Cycle::Failed("the relay node could not process an instruction"),
        }),
    };
    drop(monitor);
    relay
        .shutdown()
        .await
        .map_err(|_| shutdown_failure("the relay node", "network shutdown failed"))?;
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
