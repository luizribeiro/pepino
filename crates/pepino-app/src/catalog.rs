use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{BoardId, DeviceId, IdentityId};
use pepino_node::MemberInfo;
use serde::{Deserialize, Serialize};

use crate::app::{lock, worker};
use crate::identity::SignedEdit;
use crate::{App, AppError, BoardJoinState, LocalBoard, LocalState, Response, WireFormat};

const PREFIX: &[u8] = b"pepino-board-catalog\0";

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct CatalogState {
    revisions: BTreeMap<BoardId, Revision>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) shared: BTreeMap<BoardId, crate::SharedAccess>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) board_request: Option<crate::BoardRequest>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) joined_shares: BTreeMap<BoardId, crate::board_install::JoinedShare>,
    joins: BTreeMap<BoardId, BoardJoinState>,
    created_here: BTreeSet<BoardId>,
    bundles: BTreeMap<DeviceId, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) observed_members: BTreeMap<BoardId, BTreeSet<DeviceId>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) last_seen_ms: BTreeMap<DeviceId, i64>,
    #[serde(default)]
    pub(crate) identity: crate::identity::IdentityCatalog,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) observed_catalog_removal: Option<crate::Revocation>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) readmission_replays: BTreeSet<BoardId>,
}

impl CatalogState {
    pub(crate) fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    pub(crate) fn join(&self, id: BoardId) -> Option<BoardJoinState> {
        self.joins.get(&id).copied()
    }

    pub(crate) fn record_joins(&mut self, progress: &BTreeMap<BoardId, BoardJoinState>) {
        for (id, state) in progress {
            if let Some(previous) = self.joins.get_mut(id) {
                *previous = (*previous).max(*state);
            }
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
struct Revision {
    sequence: u64,
    writer: DeviceId,
    name: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Announcement {
    v: u32,
    identity: IdentityId,
    revisions: BTreeMap<BoardId, Revision>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    devices: BTreeMap<DeviceId, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    roster: Vec<SignedEdit>,
    member: String,
}

impl LocalState {
    pub fn created_boards(&self) -> &BTreeSet<BoardId> {
        &self.catalog.created_here
    }

    pub fn device_bundle(&self, device: DeviceId) -> Option<MemberInfo> {
        self.catalog
            .bundles
            .get(&device)
            .and_then(|info| MemberInfo::from_hex(info).ok())
    }

    pub fn encode_catalog(&self, member: &MemberInfo) -> Result<Vec<u8>, AppError> {
        let announcement = Announcement {
            v: crate::PROTOCOL_VERSION,
            identity: self.identity_id,
            revisions: self.catalog.revisions.clone(),
            devices: BTreeMap::new(),
            roster: self.catalog.identity.edits().cloned().collect(),
            member: member.to_hex(),
        };
        let mut bytes = PREFIX.to_vec();
        bytes.extend(WireFormat::Cbor.encode(&announcement)?);
        Ok(bytes)
    }

    pub(crate) fn name_board(&mut self, id: BoardId, name: &str) {
        let sequence = self
            .catalog
            .revisions
            .get(&id)
            .map_or(1, |previous| previous.sequence.saturating_add(1));
        self.merge_board(
            id,
            Revision {
                sequence,
                writer: self.device_id,
                name: name.into(),
            },
        );
    }

    fn merge_board(&mut self, id: BoardId, revision: Revision) {
        if revision.name.trim().is_empty()
            || self
                .catalog
                .revisions
                .get(&id)
                .is_some_and(|old| old >= &revision)
        {
            return;
        }
        if let Some(board) = self.boards.iter_mut().find(|board| board.id == id) {
            board.name.clone_from(&revision.name);
        } else {
            self.boards.push(LocalBoard {
                id,
                name: revision.name.clone(),
            });
            self.boards[1..].sort_unstable_by_key(|board| board.id);
            self.catalog
                .joins
                .insert(id, BoardJoinState::WaitingToBeAdded);
        }
        self.catalog.revisions.insert(id, revision);
    }

    fn merge_catalog(
        &mut self,
        board: BoardId,
        signer: DeviceId,
        announcement: Announcement,
    ) -> Result<(), AppError> {
        if board != self.catalog_board()
            || self.identity_id != announcement.identity
            || announcement.v != crate::PROTOCOL_VERSION
        {
            return Ok(());
        }
        self.catalog
            .identity
            .merge(board, self.identity_id, announcement.roster)?;
        let bootstrap_sender =
            self.roster_root().is_none() && self.other_devices.contains_key(&signer);
        if !self.active_roster_device(signer) && !bootstrap_sender {
            return Ok(());
        }
        if let Ok(info) = MemberInfo::from_hex(&announcement.member)
            && info.device_id().is_ok_and(|device| device == signer)
        {
            let saved = self
                .device_bundle(signer)
                .and_then(|info| info.expires_at().ok());
            if info.expires_at().ok() >= saved {
                self.catalog.bundles.insert(signer, announcement.member);
            }
        }
        for (id, revision) in announcement.revisions {
            self.merge_board(id, revision);
        }
        Ok(())
    }
}

impl App {
    pub async fn fresh_local_state(&self) -> Result<LocalState, AppError> {
        let directory = self.directory.clone();
        worker("reload local state", move || LocalState::load(&directory)).await?
    }

    pub async fn remember_member_bundle(&self, member: &MemberInfo) -> Result<(), AppError> {
        let id = member
            .device_id()
            .map_err(|error| crate::app::invalid("device", error))?;
        let bytes = member.to_hex();
        self.update_local(move |local| {
            if local.active_roster_device(id) {
                local.catalog.bundles.insert(id, bytes);
            }
        })
        .await
    }

    pub async fn register_created_board(
        &self,
        id: BoardId,
        name: &str,
    ) -> Result<LocalState, AppError> {
        Self::validate_board_name(name)?;
        let name = name.to_owned();
        self.update_local_state(move |local| {
            if local.catalog.created_here.insert(id) {
                local.name_board(id, &name);
                local.catalog.joins.insert(id, BoardJoinState::Joined);
            }
        })
        .await
    }

    pub async fn receive_catalog(
        &self,
        board: BoardId,
        signer: DeviceId,
        bytes: &[u8],
    ) -> Result<bool, AppError> {
        let Some(bytes) = bytes.strip_prefix(PREFIX) else {
            return Ok(false);
        };
        if let Ok(announcement) = WireFormat::Cbor.decode(bytes) {
            self.try_update_local_state(move |local| {
                local.merge_catalog(board, signer, announcement)
            })
            .await?;
        }
        Ok(true)
    }

    pub(super) async fn board_rename(
        &mut self,
        reference: &str,
        name: &str,
        guard: std::fs::File,
    ) -> Result<Response, AppError> {
        Self::validate_board_name(name)?;
        let board = self.local.resolve_board(reference, "board")?;
        let mut info = self.board_info(board).await?;
        if board.name != name {
            let mut local = self.local.clone();
            local.name_board(board.id, name);
            self.persist_boards(local, guard).await?;
            info.board.name = name.into();
        }
        Ok(Response::Board { board: info })
    }

    pub(super) async fn update_local(
        &self,
        update: impl FnOnce(&mut LocalState) + Send + 'static,
    ) -> Result<(), AppError> {
        self.update_local_state(update).await.map(|_| ())
    }

    async fn update_local_state(
        &self,
        update: impl FnOnce(&mut LocalState) + Send + 'static,
    ) -> Result<LocalState, AppError> {
        self.try_update_local_state(move |local| {
            update(local);
            Ok(())
        })
        .await
    }

    pub(super) async fn try_update_local_state(
        &self,
        update: impl FnOnce(&mut LocalState) -> Result<(), AppError> + Send + 'static,
    ) -> Result<LocalState, AppError> {
        let guard = lock(&self.directory, self.lock_timeout).await?;
        let directory = self.directory.clone();
        worker("update local state", move || {
            let _guard = guard;
            let previous = LocalState::load(&directory)?;
            let mut local = previous.clone();
            update(&mut local)?;
            if local != previous {
                local.replace(&directory)?;
            }
            Ok(local)
        })
        .await?
    }
}

#[cfg(test)]
#[test]
fn concurrent_creations_and_renames_join_independent_revisions() {
    use proptest::prelude::*;

    proptest!(|(order in proptest::collection::vec(any::<u8>(), 6))| {
        let anchor = BoardId::from_bytes([40; 32]);
        let workshop = BoardId::from_bytes([51; 32]);
        let greenhouse = BoardId::from_bytes([61; 32]);
        let laptop = DeviceId::from_bytes([71; 32]);
        let tablet = DeviceId::from_bytes([81; 32]);
        let base = LocalState::new(crate::OpenOptions::default().try_into().unwrap(), laptop, anchor).unwrap();
        let mut local = base.clone();
        let revisions = [
            (workshop, 1, laptop, "Workshop"),
            (greenhouse, 1, tablet, "Greenhouse"),
            (workshop, 2, laptop, "Wood Shop"),
            (workshop, 2, tablet, "Tool Shed"),
            (greenhouse, 2, laptop, "Seed House"),
            (workshop, 1, laptop, "Workshop"),
        ];
        let mut operations: Vec<_> = order.into_iter().zip(revisions).collect();
        operations.sort_by_key(|(sort, _)| *sort);
        for (_, (id, sequence, writer, name)) in operations {
            local.merge_board(id, Revision { sequence, writer, name: name.into() });
        }
        prop_assert_eq!(local.board_id(), anchor);
        prop_assert_eq!(local.boards()[1].name.as_str(), "Tool Shed");
        prop_assert_eq!(local.boards()[2].name.as_str(), "Seed House");
        prop_assert_eq!(local.board_join_state(greenhouse), BoardJoinState::WaitingToBeAdded);
    });
}

#[cfg(test)]
#[test]
fn ignores_legacy_device_claims_even_from_a_bootstrap_sender() {
    let board = BoardId::from_bytes([52; 32]);
    let own = DeviceId::from_bytes([53; 32]);
    let peer = DeviceId::from_bytes([54; 32]);
    let stranger = DeviceId::from_bytes([55; 32]);
    let mut local = LocalState::new(
        crate::OpenOptions::default().try_into().unwrap(),
        own,
        board,
    )
    .unwrap();
    local.other_devices.insert(peer, "Ferrum".into());
    let announcement = Announcement {
        v: crate::PROTOCOL_VERSION,
        identity: local.identity_id,
        revisions: BTreeMap::new(),
        devices: [(stranger, "Cobalt".into())].into(),
        roster: Vec::new(),
        member: String::new(),
    };
    local.merge_catalog(board, peer, announcement).unwrap();
    assert_eq!(local.devices()[&peer], "Ferrum");
    assert!(!local.devices().contains_key(&stranger));
}
