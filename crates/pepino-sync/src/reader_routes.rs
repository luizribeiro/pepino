use std::collections::BTreeSet;

use pepino_app::{BoardCards, DeviceMembershipChange, LocalState};
use pepino_core::{BoardId, DeviceId};
use pepino_node::{NodeStatus, Role};

use crate::{
    Error, Transport, TransportBoard, board_membership::apply_change, board_workers::BoardWorkers,
};

#[derive(Default)]
pub(crate) struct Routes {
    announced: BTreeSet<BoardId>,
}

impl Routes {
    pub(crate) async fn reconcile<T: Transport>(
        &mut self,
        workers: &BoardWorkers<T>,
        local: &LocalState,
        network: &NodeStatus,
    ) -> Result<(), Error> {
        self.announced
            .retain(|channel| local.card_exchanges().contains_key(channel));
        for (&channel, exchange) in local.card_exchanges() {
            if !network
                .boards
                .get(&channel)
                .is_some_and(|status| status.joined)
            {
                continue;
            }
            let (Some(handle), Some(board)) =
                (workers.handle(channel), workers.handle(exchange.board))
            else {
                continue;
            };
            if !handle.manages(local.device_id).await || !board.manages(local.device_id).await {
                continue;
            }
            let Some(person) = local.board_people(exchange.board).find(|person| {
                person.id == exchange.person
                    && exchange.root.is_none_or(|root| root == person.key().1)
            }) else {
                continue;
            };
            if !self.announced.contains(&channel) {
                let card = BoardCards::ReaderRoute {
                    channel,
                    person: person.key(),
                    incarnation: exchange.incarnation,
                };
                board.publish(card.encode()?).await?;
                self.announced.insert(channel);
            }
            let members = board.roles().await?;
            let departing: BTreeSet<_> = local
                .board_people(exchange.board)
                .filter(|person| {
                    local
                        .latest_membership(exchange.board, person.key())
                        .is_some_and(|record| {
                            !record.relayed
                                && members.get(&record.actor) == Some(&Role::Manage)
                                && record.change.action.role()
                                    != Some(pepino_app::BoardRole::Manage)
                        })
                })
                .flat_map(|person| {
                    local
                        .membership_devices(exchange.board, person.key())
                        .into_iter()
                        .filter(|device| members.get(device) == Some(&Role::Manage))
                })
                .collect();
            reconcile_channel(
                local,
                exchange.board,
                &*handle,
                &*board,
                person.key(),
                local.is_current_exchange(exchange, person.key()),
                &departing,
            )
            .await?;
        }
        Ok(())
    }
}

pub(crate) async fn reconcile_channel(
    local: &LocalState,
    board: BoardId,
    channel: &impl TransportBoard,
    handle: &impl TransportBoard,
    reader: pepino_app::PersonKey,
    active: bool,
    departing: &BTreeSet<DeviceId>,
) -> Result<(), Error> {
    let members = handle.roles().await?;
    let current = channel.roles().await?;
    let conflicts = local.membership_conflicts(board);
    let reader_devices = &(&local.membership_devices(board, reader) - &conflicts) - departing;
    let departed = !active
        || local
            .latest_membership(board, reader)
            .is_some_and(|record| record.change.action.role().is_none());
    let reader_devices = if departed {
        BTreeSet::new()
    } else {
        reader_devices
    };
    let managers: BTreeSet<_> = members
        .iter()
        .filter_map(|(&id, &role)| {
            (role == Role::Manage
                && !conflicts.contains(&id)
                && !departing.contains(&id)
                && local.board_people(board).any(|person| person.confirms(id)))
            .then_some(id)
        })
        .collect();
    let bundle_for = |device: DeviceId| {
        local.device_bundle(device).or_else(|| {
            local
                .board_people(board)
                .filter(|person| person.confirms(device))
                .find_map(|person| person.device_bundle(device))
        })
    };
    for (device, role) in managers.iter().map(|&id| (id, Role::Manage)).chain(
        reader_devices
            .difference(&managers)
            .filter(|id| members.contains_key(id))
            .map(|&id| (id, Role::Write)),
    ) {
        match current.get(&device) {
            Some(existing) if *existing != role => {
                apply_change(channel, DeviceMembershipChange::SetRole(device, role)).await?
            }
            Some(_) => (),
            None => {
                if let Some(bundle) = bundle_for(device) {
                    apply_change(
                        channel,
                        DeviceMembershipChange::Enroll(Box::new(bundle), role),
                    )
                    .await?;
                }
            }
        }
    }
    let verified = channel.roles().await?;
    let successor_available = managers
        .iter()
        .any(|id| verified.get(id) == Some(&Role::Manage));
    for (&device, &role) in &current {
        if !managers.contains(&device)
            && (!reader_devices.contains(&device) || !members.contains_key(&device))
        {
            if role == Role::Manage && !successor_available {
                continue;
            }
            apply_change(channel, DeviceMembershipChange::Remove(device)).await?;
        }
    }
    Ok(())
}

pub(crate) async fn handoff<T: Transport>(
    workers: &BoardWorkers<T>,
    local: &LocalState,
    board: BoardId,
    handle: &T::Board,
    losing_access: &BTreeSet<DeviceId>,
) -> Result<(), Error> {
    if losing_access.is_empty() {
        return Ok(());
    }
    let members = handle.roles().await?;
    let departing: BTreeSet<_> = losing_access
        .iter()
        .copied()
        .filter(|device| members.get(device) == Some(&Role::Manage))
        .collect();
    if departing.is_empty() {
        return Ok(());
    }
    for (&channel, exchange) in local
        .card_exchanges()
        .iter()
        .filter(|(_, route)| route.board == board)
    {
        let Some(person) = local
            .board_people(board)
            .find(|person| exchange.matches(person.key()))
        else {
            continue;
        };
        let Some(channel) = workers.handle(channel) else {
            return Err(Error::ReaderHandoff {
                reader: person.name().to_owned(),
            });
        };
        let current = channel.roles().await?;
        if departing.iter().all(|device| !current.contains_key(device)) {
            continue;
        }
        let failure = || Error::ReaderHandoff {
            reader: person.name().to_owned(),
        };
        if current.get(&local.device_id) != Some(&Role::Manage) {
            return Err(failure());
        }
        reconcile_channel(
            local,
            board,
            &*channel,
            handle,
            person.key(),
            local.is_current_exchange(exchange, person.key()),
            &departing,
        )
        .await?;
        if channel
            .roles()
            .await?
            .keys()
            .any(|device| departing.contains(device))
        {
            return Err(failure());
        }
    }
    Ok(())
}
