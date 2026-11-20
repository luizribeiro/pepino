use std::time::{Duration, Instant};

use futures_util::StreamExt;
use pepino_app::BoardJoinState;
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
        Operation::CreateBoard { name } => match engine.create_board(&name).await {
            Ok(board) => Outcome::BoardCreated { board },
            Err(_) => Outcome::BoardCreationFailed,
        },
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
        Operation::AddMember { device, name } => match engine.add_member(device, name).await {
            Ok(()) => Outcome::MemberAdded,
            Err(pepino_sync::Error::EnrollmentIncomplete { enrolled, failed }) => {
                Outcome::MemberAddIncomplete { enrolled, failed }
            }
            Err(pepino_sync::Error::EnrollmentAccess { board }) => {
                Outcome::MemberAccessDenied { board }
            }
            Err(_) => Outcome::error(
                "add_member_failed",
                "Could not grant board membership. Check that this device has joined every board.",
            ),
        },
        Operation::RegisterPeerHint { peer } => match engine.register_peer_hint(peer).await {
            Ok(()) => Outcome::PeerHintRegistered,
            Err(_) => Outcome::error(
                "peer_hint_failed",
                "Could not register the granting device's connection hint.",
            ),
        },
        Operation::BoardJoinState { timeout_ms } => {
            let changes = engine.status_changes();
            tokio::pin!(changes);
            let complete = async {
                while let Some(status) = changes.next().await {
                    if status
                        .board_join_states()
                        .values()
                        .all(|state| *state == BoardJoinState::Joined)
                    {
                        break;
                    }
                }
            };
            let _ = tokio::time::timeout(Duration::from_millis(timeout_ms), complete).await;
            Outcome::BoardJoinState {
                boards: engine.status().board_join_states(),
            }
        }
        Operation::Stop => Outcome::Stopping,
    }
}
