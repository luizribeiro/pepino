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

pub type MembershipEvents = BoxStream<'static, Result<MemberStates, Error>>;

pub(crate) struct MembershipMonitor {
    pub(crate) members: Arc<Mutex<MemberStates>>,
    pub(crate) changes: broadcast::Sender<MemberStates>,
}

impl MembershipMonitor {
    pub(crate) fn new(members: MemberStates) -> Self {
        Self {
            members: Arc::new(Mutex::new(members)),
            changes: broadcast::channel(128).0,
        }
    }

    pub(crate) fn update(&self, members: &HashMap<MemberId, AccessLevel>, event: &SpaceEvent<()>) {
        // Ejected carries a synthetic empty list, after a notification with the actual list.
        if matches!(event, SpaceEvent::Ejected { .. }) {
            return;
        }
        let mut current = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        for state in current.values_mut() {
            *state = MemberState::Removed;
        }
        for (device, level) in members {
            let device = DeviceId::from_bytes(*device.as_bytes());
            current.insert(device, MemberState::Member(Role::from_level(*level)));
        }
        let _ = self.changes.send(current.clone());
    }
}

impl crate::BoardHandle {
    /// Observe the current snapshot followed by membership notifications while the board is open.
    pub fn membership_events(&self) -> MembershipEvents {
        let members = self
            .membership
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let snapshot = stream::iter([Ok(members.clone())]);
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
