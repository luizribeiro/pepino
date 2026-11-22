use std::{future::pending, time::Instant};

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

pub(super) async fn cycle(
    listener: &UnixListener,
    settings: &NodeSettings,
    config: &Config,
    level: LogLevel,
    started: Instant,
    terminate: &mut Signal,
    interrupt: &mut Signal,
) -> Result<End, CliError> {
    let trusted = config
        .trusted_devices
        .value
        .iter()
        .map(|id| DeviceId::from_bytes(*id.as_bytes()))
        .collect();
    tokio::select! {
        _ = terminate.recv() => return Ok(End::Stopped),
        _ = interrupt.recv() => return Ok(End::Stopped),
        purged = Relay::purge_removed(settings) => {
            purged.map_err(|_| failure("Could not delete removed boards from the relay database."))?;
        }
    }
    let mut relay = tokio::select! {
        _ = terminate.recv() => return Ok(End::Stopped),
        _ = interrupt.recv() => return Ok(End::Stopped),
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
        _ = terminate.recv() => Ok(End::Stopped),
        _ = interrupt.recv() => Ok(End::Stopped),
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
        }) => result.map_err(socket_error),
        result = relay.run() => match result {
            Ok(RelayEnd::Restart) => Ok(End::Restart),
            Err(_) => Err(failure("The relay service could not process an instruction.")),
        },
    };
    drop(monitor);
    relay
        .shutdown()
        .await
        .map_err(|_| shutdown_failure("the relay node", "network shutdown failed"))?;
    end
}
