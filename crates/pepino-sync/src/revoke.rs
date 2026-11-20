use std::{collections::BTreeMap, time::Duration};
use tokio::time::Instant;

use crate::{Transport, TransportBoard, board_workers::BoardWorkers};
use pepino_app::LocalBoard;
use pepino_core::DeviceId;
use pepino_node::{BoardHandle, Error, Role};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Removal {
    Removed,
    ManagerRequired,
    LastManager,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BoardRemoval {
    pub board: LocalBoard,
    pub result: Removal,
}

fn removal_guard(members: &[(DeviceId, Role)], own: DeviceId, device: DeviceId) -> Option<Removal> {
    let Some((_, role)) = members.iter().find(|(id, _)| *id == device) else {
        return Some(Removal::Removed);
    };
    if *role == Role::Manage
        && !members
            .iter()
            .any(|&(id, role)| id != device && role == Role::Manage)
    {
        return Some(Removal::LastManager);
    }
    (!members.contains(&(own, Role::Manage))).then_some(Removal::ManagerRequired)
}

pub(crate) async fn remove(handle: &BoardHandle, own: DeviceId, device: DeviceId) -> Removal {
    let Ok(members) = handle.members().await else {
        return Removal::Unavailable;
    };
    if let Some(result) = removal_guard(&members, own, device) {
        return result;
    }
    match handle.remove_member(device, true).await {
        Ok(()) => Removal::Removed,
        Err(Error::LastManager) => Removal::LastManager,
        Err(Error::NotPermitted) => Removal::ManagerRequired,
        Err(_) => Removal::Unavailable,
    }
}

#[derive(Default)]
pub(crate) struct Repairs {
    published: BTreeMap<DeviceId, (pepino_app::Revocation, Instant)>,
}

impl Repairs {
    pub(crate) async fn complete_requests<T: Transport>(
        workers: &BoardWorkers<T>,
        mut local: pepino_app::LocalState,
    ) -> pepino_app::LocalState {
        let requested = local
            .roster()
            .into_values()
            .filter(|row| row.leave_requested_at.is_some());
        let own = local.device_id;
        let Some(catalog) = workers.handle(local.catalog_board()) else {
            return local;
        };
        for row in requested {
            if row.id != own
                && catalog.manages(own).await
                && let Ok(updated) = workers
                    .app
                    .record_device_revocation(row.id, std::time::SystemTime::now().into())
                    .await
            {
                local = updated;
            }
        }
        local
    }

    fn notice_pending(
        &mut self,
        id: DeviceId,
        revocation: pepino_app::Revocation,
        connected: bool,
    ) -> bool {
        let (saved, first) = self
            .published
            .entry(id)
            .or_insert_with(|| (revocation.clone(), Instant::now()));
        if *saved != revocation {
            *saved = revocation;
            *first = Instant::now();
        }
        connected && first.elapsed() < Duration::from_secs(2)
    }

    pub(crate) async fn reconcile<T: Transport>(
        &mut self,
        workers: &BoardWorkers<T>,
        local: &pepino_app::LocalState,
        network: &pepino_node::NodeStatus,
    ) {
        let roster = local.roster();
        self.published
            .retain(|id, _| roster.get(id).is_some_and(|row| row.revocation.is_some()));
        let mut reports = BTreeMap::new();
        for row in roster.into_values().filter(|row| row.id != local.device_id) {
            let Some(revocation) = row.revocation else {
                continue;
            };
            // A connected device needs time to ingest the notice before the catalog block.
            if self.notice_pending(
                row.id,
                revocation,
                network.connected_peers.contains(&row.id),
            ) {
                continue;
            }
            let mut outcomes = Vec::new();
            for board in local.boards().iter().rev() {
                let result = match workers.handle(board.id) {
                    Some(handle)
                        if network
                            .boards
                            .get(&board.id)
                            .is_some_and(|signals| signals.joined) =>
                    {
                        handle.remove_revoked(local.device_id, row.id).await
                    }
                    _ => Removal::Unavailable,
                };
                outcomes.push(BoardRemoval {
                    board: board.clone(),
                    result,
                });
            }
            outcomes.reverse();
            reports.insert(row.id, outcomes);
        }
        workers.changes.send_if_modified(|status| {
            if status.device_removals == reports {
                return false;
            }
            status.device_removals = reports;
            true
        });
    }
}

#[cfg(test)]
#[test]
fn removal_requires_manage_only_if_the_device_is_still_a_member() {
    let laptop = DeviceId::from_bytes([12; 32]);
    let tablet = DeviceId::from_bytes([38; 32]);
    let phone = DeviceId::from_bytes([46; 32]);
    let members = [(laptop, Role::Manage), (tablet, Role::Write)];
    assert_eq!(removal_guard(&members, laptop, tablet), None);
    assert_eq!(
        removal_guard(&members, tablet, laptop),
        Some(Removal::LastManager)
    );
    assert_eq!(
        removal_guard(&members, tablet, phone),
        Some(Removal::Removed)
    );
    let managers = [
        (laptop, Role::Manage),
        (phone, Role::Manage),
        (tablet, Role::Write),
    ];
    assert_eq!(
        removal_guard(&managers, tablet, laptop),
        Some(Removal::ManagerRequired)
    );
}

#[cfg(test)]
#[tokio::test(start_paused = true)]
async fn restart_revisits_a_durable_revocation_even_without_the_first_scan() {
    let device = DeviceId::from_bytes([51; 32]);
    let revocation = pepino_app::Revocation {
        by: DeviceId::from_bytes([52; 32]),
        at: std::time::SystemTime::UNIX_EPOCH.into(),
    };
    let mut restarted = Repairs::default();
    assert!(restarted.notice_pending(device, revocation.clone(), true));
    assert!(!restarted.notice_pending(device, revocation.clone(), false));
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(!restarted.notice_pending(device, revocation, true));
}
