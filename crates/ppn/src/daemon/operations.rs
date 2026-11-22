use std::time::{Duration, Instant};

use futures_util::StreamExt;
use pepino_app::BoardJoinState;
use pepino_sync::{Error, SyncEngine};

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
            Err(error) => failure(
                "board_creation_failed",
                "Could not create the board",
                &error,
            ),
        },
        Operation::Nudge => {
            engine.nudge();
            Outcome::Nudged
        }
        Operation::Status => {
            let state = engine.status();
            Outcome::Status {
                status: DaemonStatus {
                    pid: std::process::id(),
                    uptime_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                    peer_syncs: state.node.peer_syncs.clone(),
                    network: snapshot(state),
                    relay: None,
                },
            }
        }
        Operation::WaitIdle { timeout_ms } => {
            let idle = engine.wait_idle(Duration::from_millis(timeout_ms)).await;
            match idle {
                Ok(()) | Err(pepino_sync::Error::Timeout) => {
                    let state = engine.status();
                    Outcome::Synced {
                        idle: idle.is_ok(),
                        peer_syncs: state.node.peer_syncs.clone(),
                        network: snapshot(state),
                    }
                }
                Err(error) => failure("sync_unavailable", "Could not wait for sync", &error),
            }
        }
        Operation::AddMember { device, name } => match engine.add_member(device, name).await {
            Ok(()) => Outcome::MemberAdded,
            Err(pepino_sync::Error::EnrollmentIncomplete {
                enrolled,
                failed,
                causes,
            }) => Outcome::MemberAddIncomplete {
                enrolled,
                failed,
                causes,
            },
            Err(pepino_sync::Error::EnrollmentAccess { board }) => {
                Outcome::MemberAccessDenied { board }
            }
            Err(Error::App(pepino_app::AppError::PermissionDenied { reason, .. })) => {
                Outcome::error("permission_denied", &crate::render::single_line(&reason))
            }
            Err(error) => failure(
                "add_member_failed",
                "Could not grant board membership",
                &error,
            ),
        },
        Operation::DeviceRemoval { device, timeout_ms } => Outcome::DeviceRemoval {
            boards: engine
                .wait_device_removal(device, Duration::from_millis(timeout_ms))
                .await,
        },
        Operation::RegisterPeerHint { peer } => match engine.register_peer_hint(peer).await {
            Ok(()) => Outcome::PeerHintRegistered,
            Err(error) => failure(
                "peer_hint_failed",
                "Could not register the granting device's connection hint",
                &error,
            ),
        },
        Operation::BoardJoinState { timeout_ms, board } => {
            let select = |states: std::collections::BTreeMap<_, _>| {
                states
                    .into_iter()
                    .filter(|(id, _)| board.is_none_or(|target| *id == target))
                    .collect::<std::collections::BTreeMap<_, _>>()
            };
            let changes = engine.status_changes();
            tokio::pin!(changes);
            let complete = async {
                while changes.next().await.is_some() {
                    let states = select(engine.board_join_states().await?);
                    if board.is_none_or(|id| states.contains_key(&id))
                        && states
                            .values()
                            .all(|state| *state == BoardJoinState::Joined)
                    {
                        break;
                    }
                }
                Ok::<_, Error>(())
            };
            if let Ok(Err(error)) =
                tokio::time::timeout(Duration::from_millis(timeout_ms), complete).await
            {
                return failure(
                    "join_state_failed",
                    "Could not read board join state",
                    &error,
                );
            }
            match engine.board_join_states().await {
                Ok(states) => Outcome::BoardJoinState {
                    boards: select(states),
                },
                Err(error) => failure(
                    "join_state_failed",
                    "Could not read board join state",
                    &error,
                ),
            }
        }
        Operation::BoardRequest { name } => match engine.request_board(name.as_deref()).await {
            Ok(request) => Outcome::BoardRequest {
                request: Box::new(request),
            },
            Err(Error::App(error)) => Outcome::error("invalid_input", &error.to_string()),
            Err(error) => failure(
                "request_failed",
                "Could not prepare the board request",
                &error,
            ),
        },
        Operation::BoardInvite {
            board,
            request,
            role,
            relay_url,
        } => match engine
            .invite_board_with_relay(board, *request, role, relay_url)
            .await
        {
            Ok(grant) => Outcome::BoardInvited {
                grant: Box::new(grant),
            },
            Err(Error::EnrollmentAccess { board }) => Outcome::MemberAccessDenied { board },
            Err(Error::App(error)) => {
                let error = crate::error::CliError::from(error);
                Outcome::error(error.kind().code(), &error.to_string())
            }
            Err(error) => failure(
                "invite_failed",
                "Could not enroll the device and publish its identity card",
                &error,
            ),
        },
        Operation::BoardLeaveCheck {
            board,
            person,
            timeout_ms,
        } => match engine
            .check_board_leave(board, person, Duration::from_millis(timeout_ms))
            .await
        {
            Ok(risk) => Outcome::BoardLeaveChecked { risk },
            Err(Error::App(error)) => {
                let error = crate::error::CliError::from(error);
                Outcome::error(error.kind().code(), &error.to_string())
            }
            Err(error) => failure(
                "membership_failed",
                "Could not read the current board role",
                &error,
            ),
        },
        Operation::BoardManage {
            board,
            person,
            action,
            accept_risk,
        } => {
            match engine
                .manage_person(board, person, action, accept_risk)
                .await
            {
                Ok(departure) => Outcome::BoardManaged { departure },
                Err(error @ Error::MembershipIncomplete(_)) => {
                    Outcome::error("membership_incomplete", &error.to_string())
                }
                Err(Error::App(error)) => {
                    let error = crate::error::CliError::from(error);
                    Outcome::error(error.kind().code(), &error.to_string())
                }
                Err(Error::Node(
                    pepino_node::Error::NotPermitted | pepino_node::Error::LastManager,
                )) => Outcome::error(
                    "permission_denied",
                    "This device needs manage access and the board must keep a manager.",
                ),
                Err(error) => failure(
                    "membership_failed",
                    "Could not change board membership",
                    &error,
                ),
            }
        }
        Operation::RelayDrop { .. } => Outcome::error(
            "permission_denied",
            "This daemon runs a device. Use the relay's data directory for relay operations.",
        ),
        Operation::Stop => Outcome::Stopping,
    }
}

fn failure(code: &str, operation: &str, error: &Error) -> Outcome {
    let cause = std::error::Error::source(error).map_or_else(
        || error.to_string(),
        |source| {
            let message = error.to_string();
            let source = source.to_string();
            if message.contains(&source) {
                message
            } else {
                format!("{message}: {source}")
            }
        },
    );
    Outcome::error(
        code,
        &format!("{operation}: {}", crate::render::single_line(&cause)),
    )
}

#[cfg(test)]
#[test]
fn control_errors_preserve_the_transport_failure() {
    let result = failure(
        "invite_failed",
        "Could not invite",
        &Error::Node(pepino_node::Error::BoardRestartRequired(
            pepino_core::BoardId::from_bytes([41; 32]),
        )),
    );
    assert!(
        matches!(result, Outcome::Error { code, message } if code == "invite_failed" && message.contains("restart"))
    );
}
