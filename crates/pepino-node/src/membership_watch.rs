use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

use futures_util::{
    StreamExt,
    stream::{self, BoxStream},
};
use p2panda::{AccessLevel, SpaceEvent, spaces::MemberId};
use pepino_core::DeviceId;
use tokio::sync::broadcast;
use tokio_stream::wrappers::{BroadcastStream, errors::BroadcastStreamRecvError::Lagged};

use crate::{Error, MemberState, MemberStates, Role};

pub type MembershipEvents = BoxStream<'static, Result<MembershipEvent, Error>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum MembershipChange {
    Created,
    Added,
    Removed,
    RoleChanged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MembershipEvent {
    /// Actor and action for the upstream notification. The initial snapshot has no action.
    pub change: Option<(DeviceId, MembershipChange)>,
    /// Effective membership. An upstream notification can leave this list unchanged.
    pub members: MemberStates,
}

pub(crate) fn is_removed(members: &Mutex<MemberStates>, device: MemberId) -> bool {
    members
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&DeviceId::from_bytes(*device.as_bytes()))
        == Some(&MemberState::Removed)
}

pub(crate) struct MembershipMonitor {
    pub(crate) members: Arc<Mutex<MemberStates>>,
    pub(crate) changes: broadcast::Sender<MembershipEvent>,
}

impl MembershipMonitor {
    pub(crate) fn new(members: MemberStates) -> Self {
        Self {
            members: Arc::new(Mutex::new(members)),
            changes: broadcast::channel(128).0,
        }
    }

    pub(crate) fn update(&self, members: &HashMap<MemberId, AccessLevel>, event: &SpaceEvent<()>) {
        use MembershipChange::*;
        let (group, change) = match event {
            SpaceEvent::Created { groups_context, .. } => (groups_context, Created),
            SpaceEvent::Added { groups_context, .. } => (groups_context, Added),
            SpaceEvent::Removed { groups_context, .. } => (groups_context, Removed),
            SpaceEvent::Promoted { groups_context, .. }
            | SpaceEvent::Demoted { groups_context, .. } => (groups_context, RoleChanged),
            // Ejected carries a synthetic empty list, after a notification with the actual list.
            SpaceEvent::Ejected { .. } => return,
        };
        let mut current = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        for state in current.values_mut() {
            *state = MemberState::Removed;
        }
        for (device, level) in members {
            let device = DeviceId::from_bytes(*device.as_bytes());
            current.insert(device, MemberState::Member(Role::from_level(*level)));
        }
        let actor = DeviceId::from_bytes(*group.author.as_bytes());
        let _ = self.changes.send(MembershipEvent {
            change: Some((actor, change)),
            members: current.clone(),
        });
    }
}

impl crate::BoardHandle {
    /// Observe the current snapshot followed by membership notifications while the board is open.
    pub fn membership_events(&self) -> MembershipEvents {
        let members = self
            .membership
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let snapshot = stream::iter([Ok(MembershipEvent {
            change: None,
            members: members.clone(),
        })]);
        let changes = self
            .membership_changes
            .upgrade()
            .map(|sender| sender.subscribe());
        snapshot
            .chain(
                stream::iter(changes)
                    .flat_map(BroadcastStream::new)
                    .map(|item| item.map_err(|Lagged(count)| Error::MembershipLagged(count))),
            )
            .boxed()
    }
}
