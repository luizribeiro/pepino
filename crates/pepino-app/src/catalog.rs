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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) people: BTreeMap<BoardId, BTreeMap<String, crate::BoardPerson>>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) invalid_board_cards: BTreeSet<pepino_core::EventId>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) pending_cards: BTreeMap<BoardId, crate::board_card_wire::PendingCards>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) rejected_cards: BTreeMap<BoardId, std::collections::VecDeque<pepino_core::EventId>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) dropped_cards: BTreeMap<BoardId, usize>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) card_exchanges: BTreeMap<BoardId, crate::CardExchange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) board_request: Option<crate::BoardRequest>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) consumed_board_requests: BTreeMap<String, (BoardId, u64)>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) joined_shares: BTreeMap<BoardId, crate::board_install::JoinedShare>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) membership:
        BTreeMap<BoardId, BTreeMap<pepino_core::EventId, crate::MembershipRecord>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) board_leaves: BTreeMap<BoardId, BTreeMap<String, crate::SignedBoardLeave>>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) observed_board_removals: BTreeSet<BoardId>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) forgotten_boards: BTreeSet<BoardId>,
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
    pub(crate) fn forget_board(&mut self, board: BoardId) {
        self.forgotten_boards.insert(board);
        self.revisions.remove(&board);
        self.shared.remove(&board);
        self.people.remove(&board);
        self.pending_cards.remove(&board);
        self.rejected_cards.remove(&board);
        self.dropped_cards.remove(&board);
        self.card_exchanges.retain(|_, route| route.board != board);
        self.joined_shares.remove(&board);
        self.membership.remove(&board);
        self.board_leaves.remove(&board);
        self.observed_board_removals.remove(&board);
        self.joins.remove(&board);
        self.created_here.remove(&board);
        self.observed_members.remove(&board);
        self.readmission_replays.remove(&board);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    pub(crate) fn join(&self, id: BoardId) -> Option<BoardJoinState> {
        self.joins.get(&id).copied()
    }

    pub(crate) fn wait_for_board_join(&mut self, id: BoardId) {
        self.joins.insert(id, BoardJoinState::WaitingToBeAdded);
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    shared: BTreeMap<BoardId, crate::SharedAccess>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    reader_routes: BTreeMap<BoardId, crate::CardExchange>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    board_leaves: BTreeMap<BoardId, BTreeMap<String, crate::SignedBoardLeave>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    own_membership: BTreeMap<BoardId, Vec<crate::MembershipRecord>>,
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
            shared: self.catalog.shared.clone(),
            reader_routes: self
                .card_exchanges()
                .iter()
                .filter(|(_, route)| {
                    route.person == self.identity_id
                        && route
                            .root
                            .is_none_or(|root| root == self.own_person_key().1)
                })
                .map(|(&channel, &route)| (channel, route))
                .collect(),
            board_leaves: self
                .catalog
                .board_leaves
                .iter()
                .filter_map(|(&board, requests)| {
                    let requests: BTreeMap<_, _> = requests
                        .iter()
                        .filter(|(_, request)| request.leave.person == self.own_person_key())
                        .map(|(key, request)| (key.clone(), request.clone()))
                        .collect();
                    (!requests.is_empty()).then_some((board, requests))
                })
                .collect(),
            own_membership: self
                .catalog
                .shared
                .keys()
                .filter_map(|&board| {
                    let records: Vec<_> = self
                        .membership_history(board)
                        .filter(|record| {
                            record.refusal.is_none()
                                && record.change.person == self.own_person_key()
                        })
                        .cloned()
                        .collect();
                    (!records.is_empty()).then_some((board, records))
                })
                .collect(),
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
        if self.catalog.forgotten_boards.contains(&id)
            || revision.name.trim().is_empty()
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
        for (id, access) in announcement.shared {
            if id != self.catalog_board() && self.catalog.revisions.contains_key(&id) {
                let saved = self.catalog.shared.entry(id).or_insert(access);
                if saved.owner == access.owner && saved.owner_root.is_none() {
                    saved.owner_root = access.owner_root;
                }
            }
        }
        let own = self.own_person_key();
        for (channel, route) in announcement.reader_routes {
            if route.person == own.0
                && route.root.is_none_or(|root| root == own.1)
                && self.shared_access(route.board).is_some()
                && self.card_exchange_has_room(route.board, channel)
            {
                self.add_card_exchange(channel, route)?;
            }
        }
        let active_sender = self.active_roster_device(signer);
        for (board, records) in announcement.own_membership {
            if active_sender && self.shared_access(board).is_some() {
                let stored = self.catalog.membership.entry(board).or_default();
                for mut record in records
                    .into_iter()
                    .filter(|record| record.refusal.is_none() && record.change.person == own)
                {
                    record.relayed = true;
                    stored
                        .entry(record.operation)
                        .and_modify(|existing| {
                            if existing.relayed
                                && existing.change == record.change
                                && existing.actor == record.actor
                            {
                                existing.applied |= record.applied;
                            }
                        })
                        .or_insert(record);
                }
            }
        }
        for (board, requests) in announcement.board_leaves {
            if self.shared_access(board).is_some() {
                for request in requests.into_values() {
                    if request.leave.person == own
                        && request.leave.board == board
                        && request.verify()?
                    {
                        self.save_board_leave(board, request);
                    }
                }
            }
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
        self.local.check_board_write(board)?;
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
        shared: BTreeMap::new(),
        reader_routes: Default::default(),
        board_leaves: Default::default(),
        own_membership: Default::default(),
    };
    local.merge_catalog(board, peer, announcement).unwrap();
    assert_eq!(local.devices()[&peer], "Ferrum");
    assert!(!local.devices().contains_key(&stranger));
}

#[cfg(test)]
#[tokio::test]
async fn trusted_catalog_discovery_leaves_shared_access_with_the_joining_device() {
    use crate::{BoardRole, SharedAccess};
    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    let keys = app.identity_credentials().unwrap();
    app.reconcile_identity(keys.device_id(), &keys)
        .await
        .unwrap();
    let source = DeviceId::from_bytes([73; 32]);
    app.remember_device(source, "Ferrum".into(), None)
        .await
        .unwrap();
    let mut local = app.fresh_local_state().await.unwrap();
    let house = BoardId::from_bytes([74; 32]);
    let access = SharedAccess {
        owner_root: None,
        owner: IdentityId::from_bytes([75; 32]),
        role: BoardRole::Write,
        admitted: source,
    };
    let announcement = || Announcement {
        v: crate::PROTOCOL_VERSION,
        identity: local.identity_id,
        revisions: [(
            house,
            Revision {
                sequence: 1,
                writer: source,
                name: "House".into(),
            },
        )]
        .into(),
        devices: Default::default(),
        roster: Vec::new(),
        member: String::new(),
        shared: [(house, access)].into(),
        reader_routes: Default::default(),
        board_leaves: Default::default(),
        own_membership: Default::default(),
    };
    let private = local.catalog_board();
    let incoming = announcement();
    local.merge_catalog(private, source, incoming).unwrap();
    assert_eq!(local.resolve_board("House", "board").unwrap().id, house);
    assert!(local.waiting_for_access(house));
    assert!(local.shared_peers().next().is_none());
}

#[cfg(test)]
#[test]
fn the_bootstrap_catalog_sender_cannot_relay_own_membership() {
    let device = DeviceId::from_bytes([74; 32]);
    let peer = DeviceId::from_bytes([75; 32]);
    let private = BoardId::from_bytes([76; 32]);
    let shared = BoardId::from_bytes([77; 32]);
    let mut local = LocalState::new(
        crate::OpenOptions::default().try_into().unwrap(),
        device,
        private,
    )
    .unwrap();
    let person = local.own_person_key();
    local.other_devices.insert(peer, "Bootstrap tablet".into());
    local.catalog.shared.insert(
        shared,
        crate::SharedAccess {
            owner: IdentityId::from_bytes([78; 32]),
            owner_root: Some(DeviceId::from_bytes([79; 32])),
            role: crate::BoardRole::Read,
            admitted: device,
        },
    );
    let record = crate::MembershipRecord {
        operation: pepino_core::EventId::from_bytes([80; 32]),
        actor: peer,
        change: crate::MembershipChange {
            person,
            action: crate::MembershipAction::Removed,
            at: std::time::UNIX_EPOCH.into(),
            revision: 1,
            incarnation: None,
            parents: Default::default(),
        },
        refusal: None,
        applied: false,
        relayed: false,
    };
    let announcement = Announcement {
        v: crate::PROTOCOL_VERSION,
        identity: local.identity_id,
        revisions: Default::default(),
        devices: Default::default(),
        roster: Default::default(),
        member: String::new(),
        shared: Default::default(),
        reader_routes: Default::default(),
        board_leaves: Default::default(),
        own_membership: [(shared, vec![record])].into(),
    };
    local.merge_catalog(private, peer, announcement).unwrap();
    assert_eq!(local.membership_history(shared).count(), 0);
    assert_eq!(local.board_departure(shared), None);
}
