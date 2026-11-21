use std::{collections::BTreeMap, time::Duration};

use pepino_app::{
    App, AppError, BoardCards, BoardRole, DeviceMembershipChange, LocalState, MembershipAction,
    MembershipChange, MembershipRecord, PersonKey, reconcile_person,
};
use pepino_core::{BoardId, DeviceId, EventId};
use pepino_node::{NodeStatus, Role};
use tokio::time::Instant;

use crate::{Error, Transport, TransportBoard, board_workers::BoardWorkers};

#[derive(Default)]
pub(crate) struct Repairs {
    notice_since: BTreeMap<EventId, Instant>,
}

pub(crate) async fn publish_change(
    app: &App,
    local: &LocalState,
    handle: &impl TransportBoard,
    board: BoardId,
    person: PersonKey,
    action: MembershipAction,
) -> Result<EventId, Error> {
    let change = MembershipChange {
        person,
        action,
        at: std::time::SystemTime::now().into(),
        revision: local.next_membership_revision(board, person)?,
        incarnation: local.membership_incarnation(board, person),
    };
    let operation = handle
        .publish(BoardCards::Membership(change.clone()).encode()?)
        .await?;
    app.record_membership(
        board,
        MembershipRecord {
            operation,
            actor: local.device_id,
            change,
        },
    )
    .await?;
    Ok(operation)
}

impl Repairs {
    pub(crate) async fn reconcile<T: Transport>(
        &mut self,
        workers: &BoardWorkers<T>,
        local: &LocalState,
        network: &NodeStatus,
    ) -> Result<(), Error> {
        if local.check_device_write().is_err() {
            return Ok(());
        }
        for board in local.boards() {
            if local.shared_access(board.id).is_none()
                || !network
                    .boards
                    .get(&board.id)
                    .is_some_and(|status| status.joined)
            {
                continue;
            }
            let Some(handle) = workers.handle(board.id) else {
                continue;
            };
            let members = handle.roles().await?;
            if let Some(role) = members
                .get(&local.device_id)
                .copied()
                .and_then(BoardRole::from_transport)
                && local
                    .shared_access(board.id)
                    .is_some_and(|access| access.role != role)
            {
                workers.app.observe_board_role(board.id, role).await?;
            }
            if members.get(&local.device_id) != Some(&Role::Manage) {
                continue;
            }
            let conflicts = local.membership_conflicts(board.id);
            for &device in &conflicts {
                if members.contains_key(&device)
                    && let Err(error) = handle
                        .apply_device_change(DeviceMembershipChange::Remove(device))
                        .await
                {
                    crate::status::record_error(&workers.changes, board.id, error.to_string());
                }
            }
            let members = handle.roles().await?;
            let mut completed = false;
            for request in local.board_leave_requests(board.id) {
                let person = request.leave.person;
                let devices = local.membership_devices(board.id, person);
                if members.contains_key(&request.leave.device)
                    && members
                        .iter()
                        .any(|(device, role)| *role == Role::Manage && !devices.contains(device))
                    && !local
                        .latest_membership(board.id, person)
                        .is_some_and(|record| record.change.action.role().is_none())
                {
                    publish_change(
                        &workers.app,
                        local,
                        &*handle,
                        board.id,
                        person,
                        MembershipAction::Left,
                    )
                    .await?;
                    completed = true;
                }
            }
            if completed {
                continue;
            }
            for saved in local.board_people(board.id) {
                let mut person = saved.clone();
                let devices = &local.membership_devices(board.id, person.key()) - &conflicts;
                if person.key() == local.own_person_key() {
                    match local.identity_card() {
                        Ok(card) => {
                            person.merge(&card)?;
                        }
                        Err(AppError::InvalidInput { .. }) => (),
                        Err(error) => return Err(error.into()),
                    }
                }
                let current = devices
                    .get(&person.first_device)
                    .and_then(|id| members.get(id))
                    .copied()
                    .or_else(|| {
                        devices
                            .iter()
                            .find_map(|device| members.get(device).copied())
                    });
                let desired = match local.latest_membership(board.id, person.key()) {
                    Some(record) if record.change.action.role().is_none() => {
                        let connected = devices.iter().any(|device| {
                            *device != local.device_id
                                && members.contains_key(device)
                                && network.connected_peers.contains(device)
                        });
                        // Live peers can receive the departure before the transport starts blocking fresh sessions.
                        if connected {
                            let start = self
                                .notice_since
                                .entry(record.operation)
                                .or_insert_with(Instant::now);
                            if start.elapsed() < Duration::from_secs(2) {
                                continue;
                            }
                        } else {
                            self.notice_since.remove(&record.operation);
                        }
                        None
                    }
                    Some(record) => record.change.action.role().map(Into::into),
                    None => {
                        let Some(role) = current else { continue };
                        Some(role)
                    }
                };
                let mut changes = reconcile_person(&person, &devices, &members, desired);
                changes.sort_by_key(|change| changes_own_device(change, local.device_id));
                for change in changes {
                    if let Err(error) = handle.apply_device_change(change).await {
                        crate::status::record_error(&workers.changes, board.id, error.to_string());
                    }
                }
            }
        }
        Ok(())
    }
}

fn changes_own_device(change: &DeviceMembershipChange, own: DeviceId) -> bool {
    match change {
        DeviceMembershipChange::Remove(device) | DeviceMembershipChange::SetRole(device, _) => {
            *device == own
        }
        DeviceMembershipChange::Enroll(..) => false,
    }
}
