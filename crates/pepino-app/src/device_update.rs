use std::{
    collections::{BTreeMap, BTreeSet},
    time::SystemTime,
};

use pepino_core::{BoardId, DeviceId};

use crate::LocalState;

use crate::{App, AppError, BoardJoinState};

impl LocalState {
    pub fn observed_members(&self, board: BoardId) -> Option<&BTreeSet<DeviceId>> {
        self.catalog.observed_members.get(&board)
    }

    pub fn observed_last_seen(&self, device: DeviceId) -> Option<chrono::DateTime<chrono::Utc>> {
        chrono::DateTime::from_timestamp_millis(*self.catalog.last_seen_ms.get(&device)?)
    }
}

impl App {
    pub async fn record_device_observations(
        &self,
        members: BTreeMap<BoardId, BTreeSet<DeviceId>>,
        last_seen: BTreeMap<DeviceId, SystemTime>,
    ) -> Result<(), AppError> {
        if members.is_empty() && last_seen.is_empty() {
            return Ok(());
        }
        self.update_local(move |local| {
            for (board, members) in members {
                if !local.board_forgotten(board) {
                    local.catalog.observed_members.insert(board, members);
                }
            }
            local.prune_card_exchanges();
            for (id, time) in last_seen {
                let time = chrono::DateTime::<chrono::Utc>::from(time).timestamp_millis();
                if local.knows_roster_device(id) {
                    let saved = local.catalog.last_seen_ms.entry(id).or_insert(time);
                    *saved = (*saved).max(time);
                }
            }
        })
        .await
    }

    pub async fn remember_device(
        &self,
        id: DeviceId,
        fallback_name: String,
        override_name: Option<String>,
    ) -> Result<(), AppError> {
        self.record_device_admission(
            id,
            &fallback_name,
            override_name.as_deref(),
            &self.identity_credentials()?,
        )
        .await
    }

    pub async fn record_devices_seen_on_boards(
        &self,
        devices: BTreeSet<DeviceId>,
    ) -> Result<(), AppError> {
        self.update_local(move |local| {
            for id in devices {
                if local.knows_roster_device(id) {
                    local.devices_seen_on_boards.insert(id);
                }
            }
        })
        .await
    }

    pub async fn record_board_joins(
        &self,
        progress: BTreeMap<BoardId, BoardJoinState>,
    ) -> Result<(), AppError> {
        self.update_local(move |local| {
            local.catalog.record_joins(&progress);
            if let Some(link) = &mut local.link {
                for (id, state) in progress {
                    if let Some(previous) = link.progress.get_mut(&id) {
                        *previous = (*previous).max(state);
                    }
                }
            }
        })
        .await
    }
}

#[cfg(test)]
#[tokio::test]
async fn readding_a_device_keeps_its_existing_name_unless_overridden() {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    let peer = DeviceId::from_bytes([34; 32]);
    app.remember_device(peer, "tablet".into(), Some("Garden tablet".into()))
        .await
        .unwrap();
    app.remember_device(peer, "tablet".into(), None)
        .await
        .unwrap();
    let local = LocalState::load(dir.path()).unwrap();
    assert_eq!(local.devices()[&peer], "Garden tablet");
    assert_eq!(local.roster()[&peer].added_by, Some(local.device_id));
    assert!(local.roster()[&peer].added_at.is_some());
    assert!(app.remember_device(peer, "".into(), None).await.is_err());
}

#[cfg(test)]
#[tokio::test]
async fn concurrent_enrollments_keep_both_device_names() {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    let ferrum = DeviceId::from_bytes([44; 32]);
    let cobalt = DeviceId::from_bytes([55; 32]);
    tokio::try_join!(
        app.remember_device(ferrum, "Ferrum".into(), None),
        app.remember_device(cobalt, "Cobalt".into(), None),
    )
    .unwrap();
    let local = LocalState::load(dir.path()).unwrap();
    assert_eq!(local.devices()[&ferrum], "Ferrum");
    assert_eq!(local.devices()[&cobalt], "Cobalt");
}

#[cfg(test)]
#[tokio::test]
async fn stores_board_observations_only_for_known_devices() {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    let tablet = DeviceId::from_bytes([56; 32]);
    let stranger = DeviceId::from_bytes([21; 32]);
    app.remember_device(tablet, "Garden tablet".into(), None)
        .await
        .unwrap();
    app.record_devices_seen_on_boards([tablet, stranger].into())
        .await
        .unwrap();
    let state = LocalState::load(dir.path()).unwrap();
    assert!(state.saw_device_on_board(tablet));
    assert!(!state.saw_device_on_board(stranger));
}

#[cfg(test)]
#[tokio::test]
async fn preserves_the_last_membership_observation_across_opens() {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    let board = app.local_state().catalog_board();
    let peer = DeviceId::from_bytes([29; 32]);
    app.remember_device(peer, "Ferrum".into(), None)
        .await
        .unwrap();
    app.record_device_observations(
        [(board, [peer].into())].into(),
        [(peer, SystemTime::UNIX_EPOCH)].into(),
    )
    .await
    .unwrap();
    let state = LocalState::load(dir.path()).unwrap();
    assert!(state.observed_members(board).unwrap().contains(&peer));
    assert_eq!(state.observed_last_seen(peer).unwrap().timestamp(), 0);
}
