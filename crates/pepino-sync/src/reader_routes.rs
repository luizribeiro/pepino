use std::collections::BTreeSet;

use pepino_app::{BoardCards, DeviceMembershipChange, LocalState};
use pepino_core::{BoardId, DeviceId};
use pepino_node::{NodeStatus, Role};

use crate::{Error, Transport, TransportBoard, board_workers::BoardWorkers};

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
            reconcile_channel(
                local,
                exchange.board,
                &*handle,
                &*board,
                person.key(),
                local.is_current_exchange(exchange, person.key()),
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
) -> Result<(), Error> {
    let members = handle.roles().await?;
    let current = channel.roles().await?;
    let reader_devices = local.membership_devices(board, reader);
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
        .filter_map(|(&id, &role)| (role == Role::Manage).then_some(id))
        .collect();
    let bundle_for = |device: DeviceId| {
        local.device_bundle(device).or_else(|| {
            local
                .board_people(board)
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
                channel
                    .apply_device_change(DeviceMembershipChange::SetRole(device, role))
                    .await?
            }
            Some(_) => (),
            None => {
                if let Some(bundle) = bundle_for(device) {
                    channel
                        .apply_device_change(DeviceMembershipChange::Enroll(Box::new(bundle), role))
                        .await?;
                }
            }
        }
    }
    for (&device, &role) in &current {
        if !managers.contains(&device)
            && (!reader_devices.contains(&device) || !members.contains_key(&device))
        {
            if role == Role::Manage
                && !channel
                    .roles()
                    .await?
                    .iter()
                    .any(|(id, role)| *role == Role::Manage && *id != device)
            {
                continue;
            }
            channel
                .apply_device_change(DeviceMembershipChange::Remove(device))
                .await?;
        }
    }
    Ok(())
}
