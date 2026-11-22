use std::time::Duration;

use pepino_app::{
    AppError, BoardDeparture, BoardRole, CardExchange, ManagePerson, MembershipAction, PersonKey,
};
use pepino_core::BoardId;
use pepino_node::Role;

use crate::{
    Error, SyncEngine, TransportBoard,
    board_membership::{device_mismatches, publish_change},
    retry::Retry,
};

impl SyncEngine {
    pub async fn check_board_leave(
        &self,
        board: BoardId,
        person: PersonKey,
        wait: Duration,
    ) -> Result<bool, Error> {
        let unknown = || AppError::PermissionDenied {
            path: "board membership".into(),
            reason: "the current board role is unknown. Sync with a board manager and try again"
                .into(),
        };
        let updates = self.status_changes();
        use futures_util::StreamExt;
        let mut updates = std::pin::pin!(updates);
        tokio::time::timeout(wait, async {
            loop {
                {
                    let workers = self.boards.lock().await;
                    let local = self.app.fresh_local_state().await?;
                    if local.board_departure(board).is_some() {
                        return Ok(false);
                    }
                    if let Some(handle) = workers.handle(board) {
                        let members = handle.roles().await?;
                        let own = members.get(&local.device_id);
                        let only_manager = own == Some(&Role::Manage)
                            && !members
                                .iter()
                                .any(|(id, role)| *id != local.device_id && *role == Role::Manage);
                        let status = self.status();
                        if own.is_some()
                            && !local.needs_readmission_replay(board)
                            && status.node.boards.get(&board).is_some_and(|node| {
                                node.joined
                                    && !node.replaying
                                    && node.active_syncs.is_empty()
                                    && (node.synced_with_peer || only_manager)
                            })
                        {
                            let devices = local.membership_devices(board, person);
                            ManagePerson::Leave
                                .check(&local, board, person, &devices, &members, true)?;
                            return Ok(ManagePerson::Leave
                                .requires_risk_ack(&local, board, person, &devices, &members));
                        }
                    }
                }
                if updates.next().await.is_none() {
                    return Err(unknown().into());
                }
            }
        })
        .await
        .map_err(|_| unknown())?
    }

    pub async fn manage_person(
        &self,
        board: BoardId,
        person: PersonKey,
        action: ManagePerson,
        accept_risk: bool,
    ) -> Result<Option<BoardDeparture>, Error> {
        let mut workers = self.boards.lock().await;
        let local = self.app.fresh_local_state().await?;
        let known = local.resolve_board(&board.to_string(), "board")?;
        if matches!(action, ManagePerson::Leave)
            && let Some(departure) = local.board_departure(board)
        {
            return Ok(Some(if local.board_departure_confirmed(board) {
                departure
            } else {
                BoardDeparture::Leaving
            }));
        }
        local.check_invitable(board)?;
        let handle = workers
            .handle(board)
            .ok_or_else(|| AppError::PermissionDenied {
                path: format!("board {:?}", known.name),
                reason: "this device is waiting for access".into(),
            })?;
        let members = handle.roles().await?;
        let devices = local.membership_devices(board, person);
        if devices.is_empty() {
            return Err(AppError::PermissionDenied {
                path: format!("board {:?}", known.name),
                reason: "wait for the person's verified roster to sync".into(),
            }
            .into());
        }
        action.check(&local, board, person, &devices, &members, accept_risk)?;
        let latest = local.latest_membership(board, person);
        if latest.is_some_and(|record| record.change.action.role().is_none())
            && matches!(action, ManagePerson::Role(_))
        {
            return Err(AppError::PermissionDenied {
                path: format!("board {:?}", known.name),
                reason: "the person is a former member. Invite them again to restore access".into(),
            }
            .into());
        }
        if matches!(action, ManagePerson::Role(BoardRole::Read))
            && let Some(bundle) = devices
                .iter()
                .filter(|device| members.contains_key(device))
                .find_map(|&device| {
                    if person == local.own_person_key() {
                        local.device_bundle(device)
                    } else {
                        local
                            .board_people(board)
                            .find(|known| known.key() == person)
                            .and_then(|known| known.device_bundle(device))
                    }
                })
        {
            let exchange = CardExchange {
                board,
                person: person.0,
                root: Some(person.1),
                incarnation: local.membership_incarnation(board, person),
            };
            self.create_card_exchange(&mut workers, &local, exchange, &bundle)
                .await?;
        }
        let membership_action = match action {
            ManagePerson::Role(role) => MembershipAction::RoleChanged(role),
            ManagePerson::Remove => MembershipAction::Removed,
            ManagePerson::Leave => {
                if members.get(&local.device_id) != Some(&Role::Manage) {
                    self.app.request_board_leave(board).await?;
                    self.nudge();
                    return Ok(Some(BoardDeparture::Leaving));
                }
                MembershipAction::Left
            }
        };
        let operation = match latest {
            Some(record)
                if record.change.action.role() == membership_action.role()
                    && !record.relayed
                    && members.get(&record.actor) == Some(&Role::Manage) =>
            {
                record.operation
            }
            _ => {
                publish_change(
                    &self.app,
                    &local,
                    &*handle,
                    board,
                    person,
                    membership_action,
                )
                .await?
            }
        };
        drop(workers);
        self.nudge();

        let mut retry = Retry::new();
        let desired = membership_action.role().map(Into::into);
        let completion = async {
            loop {
                let local = self.app.fresh_local_state().await?;
                if let Some(record) = local
                    .membership_history(board)
                    .find(|record| record.operation == operation)
                {
                    if let Some(reason) = record.refusal {
                        return Err(Error::MembershipIncomplete(reason.to_string()));
                    }
                    if local.membership_superseded(board, record) {
                        return Err(Error::MembershipIncomplete(
                            "a removal superseded this change".into(),
                        ));
                    }
                }
                if device_mismatches(&local, board, person, desired, &handle.roles().await?)
                    .is_empty()
                {
                    return Ok(local.board_departure(board));
                }
                retry.wait().await;
            }
        };
        match tokio::time::timeout(Duration::from_secs(15), completion).await {
            Ok(result) => result,
            Err(_) => {
                let local = self.app.fresh_local_state().await?;
                let missing =
                    device_mismatches(&local, board, person, desired, &handle.roles().await?);
                let detail = self
                    .status()
                    .boards
                    .get(&board)
                    .and_then(|status| status.last_error.clone())
                    .unwrap_or_else(|| {
                        "some confirmed devices still need a manager or a current member bundle"
                            .into()
                    });
                let devices: Vec<_> = missing.iter().map(ToString::to_string).collect();
                Err(Error::MembershipIncomplete(format!(
                    "{detail}. Pending devices: {}",
                    devices.join(", ")
                )))
            }
        }
    }
}
