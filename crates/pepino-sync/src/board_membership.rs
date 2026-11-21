use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use pepino_app::{
    App, AppError, BoardCards, BoardRole, DeviceMembershipChange, LocalState, MembershipAction,
    MembershipChange, MembershipRecord, MembershipRefusal, PersonKey, RosterState,
    reconcile_person,
};
use pepino_core::{BoardId, DeviceId, EventId};
use pepino_node::{Error as NodeError, NodeStatus, Role};
use tokio::time::Instant;

use crate::{Error, Transport, TransportBoard, board_workers::BoardWorkers, retry::Retry};

#[derive(Default)]
pub(crate) struct Repairs {
    notice_since: BTreeMap<EventId, Instant>,
    failures: BTreeMap<(BoardId, DeviceId), (Instant, Retry)>,
}

pub(crate) async fn publish_change(
    app: &App,
    local: &LocalState,
    handle: &impl TransportBoard,
    board: BoardId,
    person: PersonKey,
    action: MembershipAction,
) -> Result<EventId, Error> {
    if handle.roles().await?.get(&local.device_id) != Some(&Role::Manage) {
        return Err(Error::MembershipIncomplete(
            MembershipRefusal::AuthorNoLongerManager.to_string(),
        ));
    }
    let change = MembershipChange {
        person,
        action,
        at: std::time::SystemTime::now().into(),
        revision: local.next_membership_revision(board, person),
        incarnation: local.membership_incarnation(board, person),
        parents: local.membership_parents(board, person),
    };
    let operation = handle
        .publish(BoardCards::Membership(change.clone()).encode()?)
        .await?;
    let refusal = (handle.roles().await?.get(&local.device_id) != Some(&Role::Manage))
        .then_some(MembershipRefusal::AuthorNoLongerManager);
    app.record_membership(
        board,
        MembershipRecord {
            operation,
            actor: local.device_id,
            refusal,
            applied: false,
            relayed: false,
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
            let mut resolved = false;
            for record in local
                .membership_history(board.id)
                .filter(|record| !record.relayed && !record.applied && record.refusal.is_none())
            {
                let target = record.change.person;
                let desired = record.change.action.role().map(Into::into);
                let result = if members.get(&record.actor) != Some(&Role::Manage) {
                    Err(MembershipRefusal::AuthorNoLongerManager)
                } else if !local.membership_devices(board.id, target).is_empty()
                    && !local.membership_superseded(board.id, record)
                    && device_mismatches(local, board.id, target, desired, &members).is_empty()
                {
                    Ok(())
                } else {
                    continue;
                };
                workers
                    .app
                    .mark_membership_result(board.id, record.operation, result)
                    .await?;
                resolved = true;
            }
            if resolved || members.get(&local.device_id) != Some(&Role::Manage) {
                continue;
            }

            let conflicts = local.membership_conflicts(board.id);
            for &device in &conflicts {
                if members.contains_key(&device) {
                    let _ = self
                        .apply(
                            workers,
                            board.id,
                            &*handle,
                            DeviceMembershipChange::Remove(device),
                        )
                        .await;
                }
            }
            let members = handle.roles().await?;
            let mut published_leave = false;
            for request in local.board_leave_requests(board.id) {
                let person = request.leave.person;
                let devices = local.membership_devices(board.id, person);
                if !devices.contains(&request.leave.device)
                    || conflicts.contains(&request.leave.device)
                    || !members.contains_key(&request.leave.device)
                    || local
                        .latest_membership(board.id, person)
                        .is_some_and(|record| record.change.action.role().is_none())
                {
                    continue;
                }
                if would_lose_managers(&members, &devices) {
                    crate::status::record_error(
                        &workers.changes,
                        board.id,
                        MembershipRefusal::LastManager.to_string(),
                    );
                    continue;
                }
                publish_change(
                    &workers.app,
                    local,
                    &*handle,
                    board.id,
                    person,
                    MembershipAction::Left,
                )
                .await?;
                published_leave = true;
            }
            if published_leave {
                continue;
            }

            for saved in local.board_people(board.id) {
                let mut person = saved.clone();
                if person.key() == local.own_person_key() {
                    match local.identity_card() {
                        Ok(card) => {
                            person.merge(&card)?;
                        }
                        Err(AppError::InvalidInput { .. }) => (),
                        Err(error) => return Err(error.into()),
                    }
                }

                let mut devices = local.membership_devices(board.id, person.key());
                devices.extend(person.confirmed_revoked_devices());
                let devices = &devices - &conflicts;
                let members = handle.roles().await?;
                let record = local.latest_membership(board.id, person.key());
                if record.is_some_and(|record| record.relayed) {
                    continue;
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
                let authorized =
                    record.filter(|record| members.get(&record.actor) == Some(&Role::Manage));
                let desired = if let Some(record) = authorized {
                    record.change.action.role().map(Into::into)
                } else if record.is_some_and(|record| record.change.action.role().is_none()) {
                    continue;
                } else {
                    let Some(role) = current else { continue };
                    Some(role)
                };
                let mut changes = reconcile_person(&person, &devices, &members, desired);
                let losing_manage: BTreeSet<_> = changes
                    .iter()
                    .filter_map(|change| match change {
                        DeviceMembershipChange::Remove(device) => Some(*device),
                        DeviceMembershipChange::SetRole(device, role) if *role != Role::Manage => {
                            Some(*device)
                        }
                        _ => None,
                    })
                    .collect();
                if would_lose_managers(&members, &losing_manage) {
                    if let Some(record) = authorized.filter(|record| !record.applied) {
                        workers
                            .app
                            .mark_membership_result(
                                board.id,
                                record.operation,
                                Err(MembershipRefusal::LastManager),
                            )
                            .await?;
                    }
                    crate::status::record_error(
                        &workers.changes,
                        board.id,
                        MembershipRefusal::LastManager.to_string(),
                    );
                    continue;
                }
                if let Some(record) = authorized
                    && record.change.action.role().is_none()
                    && devices.iter().any(|device| {
                        *device != local.device_id
                            && members.contains_key(device)
                            && network.connected_peers.contains(device)
                    })
                {
                    // A live member can receive the person-level notice before transport ejection.
                    let start = self
                        .notice_since
                        .entry(record.operation)
                        .or_insert_with(Instant::now);
                    if start.elapsed() < Duration::from_secs(2) {
                        continue;
                    }
                }
                if let Err(error) = crate::reader_routes::handoff(
                    workers,
                    local,
                    board.id,
                    &*handle,
                    &losing_manage,
                )
                .await
                {
                    crate::status::record_error(&workers.changes, board.id, error.to_string());
                    continue;
                }
                changes.sort_by_key(|change| {
                    changes_own_device(change, local.device_id)
                        || authorized.is_some_and(|record| changes_own_device(change, record.actor))
                });
                let mut applied_to_author = false;
                for change in changes {
                    let current = handle.roles().await?;
                    if authorized
                        .is_some_and(|record| current.get(&record.actor) != Some(&Role::Manage))
                    {
                        break;
                    }
                    let changes_author =
                        authorized.is_some_and(|record| changes_own_device(&change, record.actor));
                    match self.apply(workers, board.id, &*handle, change).await {
                        Ok(true) => applied_to_author |= changes_author,
                        Err(NodeError::LastManager)
                            if let Some(record) = authorized.filter(|record| !record.applied) =>
                        {
                            workers
                                .app
                                .mark_membership_result(
                                    board.id,
                                    record.operation,
                                    Err(MembershipRefusal::LastManager),
                                )
                                .await?;
                            break;
                        }
                        _ => (),
                    }
                }
                if let Some(record) = authorized.filter(|record| !record.applied) {
                    let current = handle.roles().await?;
                    // An authorized change can itself end its author's manager role.
                    if (applied_to_author || current.get(&record.actor) == Some(&Role::Manage))
                        && device_mismatches(local, board.id, person.key(), desired, &current)
                            .is_empty()
                    {
                        self.notice_since.remove(&record.operation);
                        workers
                            .app
                            .mark_membership_result(board.id, record.operation, Ok(()))
                            .await?;
                    }
                }
            }
        }
        Ok(())
    }
    async fn apply<T: Transport>(
        &mut self,
        workers: &BoardWorkers<T>,
        board: BoardId,
        handle: &T::Board,
        change: DeviceMembershipChange,
    ) -> Result<bool, NodeError> {
        let device = match &change {
            DeviceMembershipChange::Enroll(info, _) => info.device_id()?,
            DeviceMembershipChange::Remove(device) | DeviceMembershipChange::SetRole(device, _) => {
                *device
            }
        };
        let key = (board, device);
        if self
            .failures
            .get(&key)
            .is_some_and(|(ready, _)| *ready > Instant::now())
        {
            return Ok(false);
        }
        match apply_change(handle, change).await {
            Ok(()) => {
                self.failures.remove(&key);
                Ok(true)
            }
            Err(error) => {
                let (ready, delay) = self
                    .failures
                    .entry(key)
                    .or_insert_with(|| (Instant::now(), Retry::new()));
                *ready = Instant::now() + delay.next_delay();
                crate::status::record_error(&workers.changes, board, error.to_string());
                Err(error)
            }
        }
    }
}

pub(crate) async fn apply_change(
    handle: &impl TransportBoard,
    change: DeviceMembershipChange,
) -> Result<(), NodeError> {
    let enrollment = match &change {
        DeviceMembershipChange::Enroll(info, role) => Some((info.device_id()?, *role)),
        _ => None,
    };
    match handle.apply_device_change(change).await {
        Err(NodeError::DifferentDirectRole) if let Some((device, role)) = enrollment => {
            if handle
                .roles()
                .await?
                .get(&device)
                .is_some_and(|current| *current != role)
            {
                handle
                    .apply_device_change(DeviceMembershipChange::SetRole(device, role))
                    .await
            } else {
                Ok(())
            }
        }
        Err(NodeError::NotMember) => Ok(()),
        result => result,
    }
}

fn would_lose_managers(members: &BTreeMap<DeviceId, Role>, leaving: &BTreeSet<DeviceId>) -> bool {
    members
        .iter()
        .any(|(device, role)| *role == Role::Manage && leaving.contains(device))
        && !members
            .iter()
            .any(|(device, role)| *role == Role::Manage && !leaving.contains(device))
}

pub(crate) fn device_mismatches(
    local: &LocalState,
    board: BoardId,
    person: PersonKey,
    role: Option<Role>,
    members: &BTreeMap<DeviceId, Role>,
) -> Vec<DeviceId> {
    let conflicts = local.membership_conflicts(board);
    let active: BTreeSet<_> = if person == local.own_person_key() {
        local
            .roster()
            .into_values()
            .filter(|row| row.state == RosterState::Active)
            .map(|row| row.id)
            .collect()
    } else {
        local
            .board_people(board)
            .filter(|known| known.key() == person)
            .flat_map(|known| {
                known
                    .devices()
                    .filter(|row| row.state == RosterState::Active)
            })
            .map(|row| row.id)
            .collect()
    };
    let mut devices = local.membership_devices(board, person);
    for known in local
        .board_people(board)
        .filter(|known| known.key() == person)
    {
        devices.extend(known.confirmed_revoked_devices());
    }
    devices
        .into_iter()
        .filter(|device| {
            let expected = role.filter(|_| active.contains(device));
            (expected.is_some() && conflicts.contains(device))
                || members.get(device).copied() != expected
        })
        .collect()
}

fn changes_own_device(change: &DeviceMembershipChange, own: DeviceId) -> bool {
    match change {
        DeviceMembershipChange::Remove(device) | DeviceMembershipChange::SetRole(device, _) => {
            *device == own
        }
        DeviceMembershipChange::Enroll(..) => false,
    }
}
