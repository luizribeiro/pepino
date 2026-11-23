use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{BoardId, DeviceId, IdentityId};
use pepino_node::MemberInfo;
use serde::{Deserialize, Serialize};

use crate::app::{lock, worker};
use crate::identity::SignedEdit;
use crate::preferences::Preferences;
use crate::{App, AppError, BoardJoinState, LocalBoard, LocalState, Response, WireFormat};

pub const CATALOG_UPGRADE_NOTICE: &str =
    "another of your devices runs a newer pepino; upgrade this one";

const PREFIX: &[u8] = b"pepino-board-catalog\0";

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct CatalogState {
    revisions: BTreeMap<BoardId, Revision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    newer_catalog: Option<u32>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "crate::board_relays::read_local_relays"
    )]
    pub(crate) relays: crate::board_relays::LocalRelayCatalog,
    #[serde(
        default,
        skip_serializing_if = "crate::preferences::Preferences::is_empty"
    )]
    pub(crate) preferences: crate::preferences::Preferences,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(super) written_views: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "crate::views::SavedViews::is_empty")]
    pub(super) migrated_views: crate::views::SavedViews,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) card_exchange_order: Vec<BoardId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) board_request: Option<crate::BoardRequest>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) consumed_board_requests: BTreeMap<String, (Option<BoardId>, u64)>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) joined_shares: BTreeMap<BoardId, crate::board_install::JoinedShare>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "crate::board_membership::history::read_stored_membership"
    )]
    pub(crate) membership:
        BTreeMap<BoardId, BTreeMap<pepino_core::EventId, crate::MembershipRecord>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) refused_membership_order:
        BTreeMap<BoardId, std::collections::VecDeque<pepino_core::EventId>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) dropped_membership_records: BTreeMap<BoardId, usize>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) observed_board_removals: BTreeSet<BoardId>,
    #[serde(default, rename = "forgotten_boards", skip_serializing)]
    legacy_forgotten_boards: BTreeSet<BoardId>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) board_cleanup: BTreeMap<BoardId, crate::board_membership::forget::BoardCleanup>,
    #[serde(
        default,
        rename = "board_presence_register",
        skip_serializing_if = "Preferences::is_empty"
    )]
    board_presence: Preferences,
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
    pub(crate) fn forget_board(&mut self, board: BoardId, device: DeviceId) {
        self.revisions.remove(&board);
        self.prune_forgotten_board_relays(board, device);
        for (used_for, _) in self.consumed_board_requests.values_mut() {
            if *used_for == Some(board) {
                *used_for = None;
            }
        }
        self.shared.remove(&board);
        self.people.remove(&board);
        self.pending_cards.remove(&board);
        self.rejected_cards.remove(&board);
        self.dropped_cards.remove(&board);
        self.card_exchanges.retain(|_, route| route.board != board);
        self.card_exchange_order
            .retain(|channel| self.card_exchanges.contains_key(channel));
        self.joined_shares.remove(&board);
        self.membership.remove(&board);
        self.refused_membership_order.remove(&board);
        self.dropped_membership_records.remove(&board);
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

fn presence_key(board: BoardId) -> String {
    format!("board/{board}")
}

fn presence_board(key: &str) -> Option<BoardId> {
    let encoded = key.strip_prefix("board/")?;
    let board: BoardId = encoded.parse().ok()?;
    (board.to_string() == encoded).then_some(board)
}

#[derive(Serialize, Deserialize)]
struct Announcement {
    v: u32,
    identity: IdentityId,
    revisions: BTreeMap<BoardId, Revision>,
    #[serde(
        default,
        rename = "board_presence_register",
        skip_serializing_if = "Preferences::is_empty"
    )]
    board_presence: Preferences,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    devices: BTreeMap<DeviceId, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    roster: Vec<SignedEdit>,
    member: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    shared: BTreeMap<BoardId, crate::SharedAccess>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    reader_routes: BTreeMap<BoardId, crate::CardExchange>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "crate::board_membership::history::read_catalog_membership"
    )]
    own_membership: BTreeMap<BoardId, Vec<crate::MembershipRecord>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    relays: crate::board_relays::RelayCatalog,
    #[serde(
        default,
        skip_serializing_if = "crate::preferences::Preferences::is_empty"
    )]
    preferences: crate::preferences::Preferences,
}

impl LocalState {
    pub fn newer_catalog_version(&self) -> Option<u32> {
        self.catalog
            .newer_catalog
            .filter(|&version| version > crate::PROTOCOL_VERSION)
    }

    fn trusts_catalog_sender(&self, signer: DeviceId) -> bool {
        self.active_roster_device(signer)
            || (self.roster_root().is_none() && self.other_devices.contains_key(&signer))
    }

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
        let mut preferences = self.catalog.preferences.clone();
        let now = crate::wall_time_ms();
        preferences.retain_valid(now);
        preferences.retain_writers(|writer| self.knows_roster_device(writer));
        let mut board_presence = self.catalog.board_presence.clone();
        board_presence.retain_valid(now);
        board_presence.retain_writers(|writer| self.knows_roster_device(writer));
        let announcement = Announcement {
            v: crate::PROTOCOL_VERSION,
            identity: self.identity_id,
            revisions: self.catalog.revisions.clone(),
            board_presence,
            devices: BTreeMap::new(),
            roster: self.catalog.identity.edits().cloned().collect(),
            member: member.to_hex(),
            relays: self.own_relay_requests(),
            preferences,
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

    pub fn board_forgotten(&self, board: BoardId) -> bool {
        self.catalog
            .board_presence
            .get(&presence_key(board))
            .is_some_and(|entry| entry.value.is_none())
    }

    pub(crate) fn change_board_presence(
        &mut self,
        board: BoardId,
        forgotten: bool,
        now: u64,
    ) -> Result<(), AppError> {
        self.catalog.board_presence.write(
            presence_key(board),
            (!forgotten).then(|| "present".into()),
            self.device_id,
            now,
        )
    }

    pub(crate) fn migrate_board_presence(&mut self) -> Result<(), AppError> {
        for board in std::mem::take(&mut self.catalog.legacy_forgotten_boards) {
            if self
                .catalog
                .board_presence
                .get(&presence_key(board))
                .is_none()
            {
                self.change_board_presence(board, true, 0)?;
                self.catalog
                    .board_cleanup
                    .entry(board)
                    .or_insert(crate::board_membership::forget::BoardCleanup::Transport);
            }
        }
        Ok(())
    }

    fn merge_board_presence(
        &mut self,
        incoming: &Preferences,
        revisions: &BTreeMap<BoardId, Revision>,
        now: u64,
    ) {
        let previous = self.catalog.board_presence.clone();
        let mut allowed = incoming.clone();
        allowed.retain_entries(|key, entry| {
            let Some(id) = presence_board(key) else {
                return false;
            };
            id != self.catalog_board()
                && self.knows_roster_device(entry.writer)
                && match entry.value.as_deref() {
                    None => true,
                    Some("present") => revisions.contains_key(&id),
                    Some(_) => false,
                }
        });
        self.catalog.board_presence.merge(allowed, now);
        let changes: Vec<_> = self
            .catalog
            .board_presence
            .iter()
            .filter(|(key, entry)| previous.get(key) != Some(*entry))
            .filter_map(|(key, entry)| Some((presence_board(key)?, entry.value.is_none())))
            .collect();
        for (id, forgotten) in changes {
            if forgotten {
                self.forget_board(id, now);
            } else if previous
                .get(&presence_key(id))
                .is_some_and(|entry| entry.value.is_none())
                || self.board_removed(id)
            {
                self.cancel_pending_board_purge(id);
                self.catalog.readmission_replays.insert(id);
            }
        }
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
        if self.board_forgotten(id)
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
        now: u64,
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
        if !self.trusts_catalog_sender(signer) {
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
        if self.active_roster_device(signer) {
            self.merge_board_presence(&announcement.board_presence, &announcement.revisions, now);
        }
        let current = |local: &Self, id: BoardId| {
            let key = presence_key(id);
            announcement.board_presence.get(&key) == local.catalog.board_presence.get(&key)
        };
        for (id, revision) in announcement.revisions {
            if current(self, id) {
                self.merge_board(id, revision);
            }
        }
        for (id, access) in announcement.shared {
            if id != self.catalog_board()
                && current(self, id)
                && self.catalog.revisions.contains_key(&id)
            {
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
                && current(self, route.board)
                && self.shared_access(route.board).is_some()
            {
                self.add_card_exchange(channel, route)?;
            }
        }
        let active_sender = self.active_roster_device(signer);
        // A device linked while its inviter is offline only ever sees the inviter's
        // announcements as a bootstrap sender, so relay requests cannot wait for the roster.
        if active_sender || self.granted_by(signer) {
            self.merge_relays(signer, announcement.relays, now);
        }
        let mut preferences = announcement.preferences;
        preferences.retain_writers(|writer| self.knows_roster_device(writer));
        let echoed = self.migration_echoes(&preferences, now);
        self.catalog.preferences.merge(preferences, now);
        self.settle_view_migrations(&echoed, now);
        self.catalog
            .written_views
            .retain(|key| self.catalog.preferences.get(key).is_some());
        for (board, records) in announcement.own_membership {
            if active_sender && current(self, board) && self.shared_access(board).is_some() {
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
        now: u64,
    ) -> Result<bool, AppError> {
        let Some(bytes) = bytes.strip_prefix(PREFIX) else {
            return Ok(false);
        };
        let wire = WireFormat::Cbor;
        match wire.decode(bytes) {
            Ok(announcement) => {
                let guard = lock(&self.directory, self.lock_timeout).await?;
                let local = self.fresh_local_state().await?;
                let (previous, guard) = self.resume_board_history(local, guard).await?;
                let mut local = previous.clone();
                local.merge_catalog(board, signer, announcement, now)?;
                if local != previous {
                    self.save_and_resume_board_cleanup(local, guard).await?;
                }
            }
            Err(AppError::UnsupportedVersion {
                requested,
                supported,
            }) if requested > supported => {
                #[derive(Deserialize)]
                struct Header {
                    identity: IdentityId,
                }
                if let Ok(Header { identity }) = wire.decode_value(bytes) {
                    self.update_local(move |local| {
                        if board == local.catalog_board()
                            && identity == local.identity_id
                            && local.trusts_catalog_sender(signer)
                        {
                            local.catalog.newer_catalog =
                                local.catalog.newer_catalog.max(Some(requested));
                        }
                    })
                    .await?;
                }
            }
            Err(_) => {}
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
            self.persist_local(local, guard).await?;
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
        board_presence: Default::default(),
        v: crate::PROTOCOL_VERSION,
        identity: local.identity_id,
        revisions: BTreeMap::new(),
        devices: [(stranger, "Cobalt".into())].into(),
        roster: Vec::new(),
        member: String::new(),
        shared: BTreeMap::new(),
        reader_routes: Default::default(),
        own_membership: Default::default(),
        relays: Default::default(),
        preferences: Default::default(),
    };
    local.merge_catalog(board, peer, announcement, 0).unwrap();
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
        board_presence: Default::default(),
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
        own_membership: Default::default(),
        relays: Default::default(),
        preferences: Default::default(),
    };
    let private = local.catalog_board();
    let incoming = announcement();
    local.merge_catalog(private, source, incoming, 0).unwrap();
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
        board_presence: Default::default(),
        v: crate::PROTOCOL_VERSION,
        identity: local.identity_id,
        revisions: Default::default(),
        devices: Default::default(),
        roster: Default::default(),
        member: String::new(),
        shared: Default::default(),
        reader_routes: Default::default(),
        own_membership: [(shared, vec![record])].into(),
        relays: Default::default(),
        preferences: Default::default(),
    };
    local.merge_catalog(private, peer, announcement, 0).unwrap();
    assert_eq!(local.membership_history(shared).count(), 0);
    assert!(!local.board_removed(shared));
}

#[cfg(test)]
#[test]
fn a_newly_linked_device_keeps_relay_requests_from_its_bootstrap_sender() {
    use crate::{BoardRelay, RelayState};
    use pepino_node::RelayRevision;

    let dir = tempfile::tempdir().unwrap();
    let relay = pepino_node::DeviceCredentials::load_or_create(dir.path().join("relay.cbor"))
        .unwrap()
        .device_id();
    let tablet = DeviceId::from_bytes([74; 32]);
    let laptop = DeviceId::from_bytes([75; 32]);
    let phone = DeviceId::from_bytes([77; 32]);
    let stranger = DeviceId::from_bytes([81; 32]);
    let greenhouse = BoardId::from_bytes([76; 32]);
    let mut local = LocalState::new(
        crate::OpenOptions::default().try_into().unwrap(),
        tablet,
        greenhouse,
    )
    .unwrap();
    local.other_devices.insert(laptop, "Laptop".into());
    local.other_devices.insert(phone, "Phone".into());
    local.link = Some(Box::new(crate::device_grant::Link {
        request: "request binding".into(),
        peer_id: laptop,
        relay_url: None,
        progress: Default::default(),
    }));
    assert_eq!(local.roster_root(), None);
    let request = |writer| BoardRelay {
        revision: RelayRevision {
            sequence: 1_795_300_000_000,
            writer,
        },
        state: RelayState::Carrying,
        carry: true,
        relay_url: Some("https://relay.example/".into()),
        reason: None,
        possible_copy: None,
    };
    let identity = local.identity_id;
    let announcement = |writer| Announcement {
        board_presence: Default::default(),
        v: crate::PROTOCOL_VERSION,
        identity,
        revisions: Default::default(),
        devices: Default::default(),
        roster: Default::default(),
        member: String::new(),
        shared: Default::default(),
        reader_routes: Default::default(),
        own_membership: Default::default(),
        relays: [(greenhouse, [(relay, request(writer))].into())].into(),
        preferences: Default::default(),
    };
    let now = 1_795_300_060_000;
    for outsider in [stranger, phone] {
        let incoming = announcement(outsider);
        local
            .merge_catalog(greenhouse, outsider, incoming, now)
            .unwrap();
        assert_eq!(local.board_relays(greenhouse).count(), 0);
    }
    let from_laptop = announcement(laptop);
    local
        .merge_catalog(greenhouse, laptop, from_laptop, now)
        .unwrap();
    assert_eq!(
        local.board_relays(greenhouse).collect::<Vec<_>>(),
        [(relay, &request(laptop))]
    );
}

#[cfg(test)]
fn preference_announcement(
    local: &LocalState,
    preferences: crate::preferences::Preferences,
) -> Announcement {
    Announcement {
        board_presence: Default::default(),
        v: crate::PROTOCOL_VERSION,
        identity: local.identity_id,
        revisions: Default::default(),
        devices: Default::default(),
        roster: Default::default(),
        member: String::new(),
        shared: Default::default(),
        reader_routes: Default::default(),
        own_membership: Default::default(),
        relays: Default::default(),
        preferences,
    }
}

#[cfg(test)]
#[test]
fn bootstrap_accepts_relayed_preferences_only_from_known_writers() {
    let tablet = DeviceId::from_bytes([74; 32]);
    let laptop = DeviceId::from_bytes([75; 32]);
    let stranger = DeviceId::from_bytes([81; 32]);
    let board = BoardId::from_bytes([76; 32]);
    let now = 1_800_000_000_000;
    let mut local = LocalState::new(
        crate::OpenOptions::default().try_into().unwrap(),
        tablet,
        board,
    )
    .unwrap();
    local.other_devices.insert(laptop, "Laptop".into());
    let mut prefs = crate::preferences::Preferences::default();
    prefs
        .write(
            "view/foreign".into(),
            Some(r#"{"filter":"tag:garden"}"#.into()),
            stranger,
            now,
        )
        .unwrap();
    prefs
        .write(
            "view/known".into(),
            Some(r#"{"filter":"tag:hardware"}"#.into()),
            tablet,
            now,
        )
        .unwrap();
    local
        .merge_catalog(
            board,
            stranger,
            preference_announcement(&local, prefs.clone()),
            now,
        )
        .unwrap();
    assert!(local.catalog.preferences.is_empty());
    local
        .merge_catalog(board, laptop, preference_announcement(&local, prefs), now)
        .unwrap();
    assert!(local.catalog.preferences.get("view/foreign").is_none());
    assert_eq!(
        local.catalog.preferences.get("view/known").unwrap().writer,
        tablet
    );
}

#[cfg(test)]
#[test]
fn two_devices_recover_both_legacy_filters_after_a_catalog_exchange() {
    let board = BoardId::from_bytes([76; 32]);
    let now = 1_800_000_000_000;
    let mut states: Vec<LocalState> = [
        (74, 75, "tag:garden", now),
        (75, 74, "tag:hardware", now + 1),
    ]
    .into_iter()
    .map(|(id, peer, filter, stamp)| {
        let mut local = LocalState::new(
            crate::OpenOptions::default().try_into().unwrap(),
            DeviceId::from_bytes([id; 32]),
            board,
        )
        .unwrap();
        local
            .other_devices
            .insert(DeviceId::from_bytes([peer; 32]), "Other device".into());
        local.saved_views.insert("weekend".into(), filter.into());
        local
            .saved_views
            .insert("identical".into(), "is:open".into());
        local.migrate_views(stamp);
        local
    })
    .collect();
    states[1].identity_id = states[0].identity_id;
    for (receiver, sender) in [(0, 1), (1, 0), (0, 1), (1, 0)] {
        let signer = states[sender].device_id;
        let announcement =
            preference_announcement(&states[sender], states[sender].catalog.preferences.clone());
        states[receiver]
            .merge_catalog(board, signer, announcement, now + 2)
            .unwrap();
    }
    for local in &states {
        let views = local.view_filters();
        assert_eq!(views.get("weekend"), Some("tag:hardware"));
        assert!(
            views
                .iter()
                .any(|(name, filter)| name.starts_with("weekend-from-") && filter == "tag:garden")
        );
        assert!(local.catalog.migrated_views.is_empty());
        assert!(!local.catalog.written_views.contains("view/identical"));
    }
}

#[cfg(test)]
#[tokio::test]
async fn recognizes_newer_catalogs_before_decoding_their_body() {
    #[derive(Serialize)]
    struct FutureCatalog {
        v: u32,
        identity: IdentityId,
        future_field: &'static str,
    }

    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let local = app.local_state();
    let board = local.catalog_board();
    let writer = local.device_id;
    let body = FutureCatalog {
        v: crate::PROTOCOL_VERSION + 1,
        identity: local.identity_id,
        future_field: "garden supplies",
    };
    let mut bytes = PREFIX.to_vec();
    bytes.extend(WireFormat::Cbor.encode(&body).unwrap());

    app.receive_catalog(board, DeviceId::from_bytes([81; 32]), &bytes, 0)
        .await
        .unwrap();
    app.receive_catalog(BoardId::from_bytes([82; 32]), writer, &bytes, 0)
        .await
        .unwrap();
    assert_eq!(
        app.fresh_local_state()
            .await
            .unwrap()
            .newer_catalog_version(),
        None
    );
    for _ in 0..2 {
        assert!(app.receive_catalog(board, writer, &bytes, 0).await.unwrap());
    }
    assert_eq!(
        app.fresh_local_state()
            .await
            .unwrap()
            .newer_catalog_version(),
        Some(crate::PROTOCOL_VERSION + 1)
    );
}

#[cfg(test)]
#[tokio::test]
async fn a_catalog_tombstone_wins_over_old_announcements_until_readmission() {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    let keys = app.identity_credentials().unwrap();
    let mut source = app
        .reconcile_identity(keys.device_id(), &keys)
        .await
        .unwrap();
    let private = source.catalog_board();
    let workshop = BoardId::from_bytes([83; 32]);
    let greenhouse = BoardId::from_bytes([84; 32]);
    source.name_board(workshop, "Workshop");
    source.name_board(greenhouse, "Greenhouse");
    let announcement = |source: &LocalState| Announcement {
        board_presence: source.catalog.board_presence.clone(),
        revisions: source.catalog.revisions.clone(),
        ..preference_announcement(source, Default::default())
    };
    let mut sibling = source.clone();
    source.change_board_presence(workshop, true, 1000).unwrap();
    source.forget_board(workshop, 1000);
    let stale = sibling.clone();
    for snapshot in [&source, &stale] {
        sibling
            .merge_catalog(private, source.device_id, announcement(snapshot), 1000)
            .unwrap();
    }
    assert!(sibling.board_forgotten(workshop));
    assert!(sibling.resolve_board("Workshop", "board").is_err());
    assert_eq!(
        sibling.resolve_board("Greenhouse", "board").unwrap().id,
        greenhouse
    );

    let forgotten = source.clone();
    source.change_board_presence(workshop, false, 3000).unwrap();
    source.name_board(workshop, "Workshop");
    for snapshot in [&source, &forgotten] {
        sibling
            .merge_catalog(private, source.device_id, announcement(snapshot), 1000)
            .unwrap();
    }
    assert!(!sibling.board_forgotten(workshop));
    assert_eq!(
        sibling.resolve_board("Workshop", "board").unwrap().id,
        workshop
    );
    assert!(sibling.needs_readmission_replay(workshop));
}

#[cfg(test)]
mod presence_tests;
