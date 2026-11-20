use std::time::{Duration, Instant};

use pepino_sync::SyncEngine;

use super::{
    network::snapshot,
    protocol::{DaemonStatus, Operation, Outcome},
};

pub(super) async fn operate(
    engine: &SyncEngine,
    operation: Operation,
    started: Instant,
) -> Outcome {
    match operation {
        Operation::Nudge => {
            engine.nudge();
            Outcome::Nudged
        }
        Operation::Status => Outcome::Status {
            status: DaemonStatus {
                pid: std::process::id(),
                uptime_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                network: snapshot(engine.status()),
            },
        },
        Operation::WaitIdle { timeout_ms } => {
            let idle = engine.wait_idle(Duration::from_millis(timeout_ms)).await;
            match idle {
                Ok(()) | Err(pepino_sync::Error::Timeout) => Outcome::Synced {
                    idle: idle.is_ok(),
                    network: snapshot(engine.status()),
                },
                Err(_) => Outcome::error("sync_unavailable", "Sync is restarting."),
            }
        }
        Operation::Stop => Outcome::Stopping,
    }
}
