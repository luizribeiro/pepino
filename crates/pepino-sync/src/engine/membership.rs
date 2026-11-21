use pepino_app::{
    AppError, BoardDeparture, BoardRole, CardExchange, ManagePerson, MembershipAction, PersonKey,
};
use pepino_core::BoardId;
use pepino_node::Role;

use crate::{Error, SyncEngine, TransportBoard, board_membership::publish_change};

impl SyncEngine {
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
        if matches!(action, ManagePerson::Leave) && local.board_departure(board).is_some() {
            return Ok(local.board_departure(board));
        }
        local.check_invitable(board)?;
        let handle = workers
            .handle(board)
            .ok_or_else(|| AppError::PermissionDenied {
                path: format!("board {:?}", known.name),
                reason: "this device is waiting for access".into(),
            })?;
        let members = handle.roles().await?;
        let mut devices = local.membership_devices(board, person);
        if person == local.own_person_key() {
            devices.extend(local.roster().into_keys());
        }
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
            && !local
                .card_exchanges()
                .values()
                .any(|route| route.board == board && local.is_current_exchange(route, person))
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
        if action.affects_managers(&devices, &members)
            && members.get(&local.device_id) == Some(&Role::Manage)
        {
            let network = workers.node.status();
            crate::reader_routes::Routes::default()
                .reconcile(&workers, &local, &network)
                .await?;
        }
        match action {
            ManagePerson::Role(role) => {
                if !latest.is_some_and(|record| record.change.action.role() == Some(role)) {
                    publish_change(
                        &self.app,
                        &local,
                        &*handle,
                        board,
                        person,
                        MembershipAction::RoleChanged(role),
                    )
                    .await?;
                }
            }
            ManagePerson::Remove => {
                if !latest.is_some_and(|record| record.change.action.role().is_none()) {
                    publish_change(
                        &self.app,
                        &local,
                        &*handle,
                        board,
                        person,
                        MembershipAction::Removed,
                    )
                    .await?;
                }
            }
            ManagePerson::Leave => {
                if members.get(&local.device_id) == Some(&Role::Manage) {
                    publish_change(
                        &self.app,
                        &local,
                        &*handle,
                        board,
                        person,
                        MembershipAction::Left,
                    )
                    .await?;
                } else {
                    self.app.request_board_leave(board).await?;
                }
            }
        }
        self.nudge();
        Ok(self.app.fresh_local_state().await?.board_departure(board))
    }
}
