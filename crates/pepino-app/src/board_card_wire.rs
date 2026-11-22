use std::collections::{BTreeMap, VecDeque};

use pepino_core::{BoardId, DeviceId, EventId};
use serde::{Deserialize, Serialize};

use crate::{App, AppError, BoardAdmission, IdentityCard, LocalState, WireFormat};

const PENDING_COUNT: usize = 64;
const PENDING_BYTES: usize = 256 * 1024;
const REJECTED_BYTES: usize = 16 * 1024;
// A rejected entry encodes as 64 hex digits, quotes and a separator.
const REJECTED_COUNT: usize = REJECTED_BYTES / 67;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(from = "PendingEncoding")]
pub(crate) struct PendingCards(VecDeque<(EventId, IdentityCard)>);

#[derive(Deserialize)]
#[serde(untagged)]
enum PendingEncoding {
    Ordered(VecDeque<(EventId, IdentityCard)>),
    Legacy(BTreeMap<EventId, IdentityCard>),
}

impl From<PendingEncoding> for PendingCards {
    fn from(value: PendingEncoding) -> Self {
        Self(match value {
            PendingEncoding::Ordered(ordered) => ordered,
            PendingEncoding::Legacy(legacy) => legacy.into_iter().collect(),
        })
    }
}

impl PendingCards {
    fn trim(&mut self) -> usize {
        let before = self.0.len();
        let mut bytes = 0_usize;
        let keep = self
            .0
            .iter()
            .rev()
            .take(PENDING_COUNT)
            .take_while(|entry| {
                bytes = bytes.saturating_add(
                    serde_json::to_vec(entry).map_or(usize::MAX, |raw| raw.len() + 1),
                );
                bytes < PENDING_BYTES
            })
            .count();
        self.0.drain(..before - keep);
        before - keep
    }

    pub(crate) fn into_entries(self) -> impl Iterator<Item = (EventId, IdentityCard)> {
        self.0.into_iter()
    }
}

impl crate::catalog::CatalogState {
    pub(crate) fn queue_card(&mut self, board: BoardId, id: EventId, card: IdentityCard) {
        if serde_json::to_vec(&(id, &card)).map_or(true, |bytes| bytes.len() + 2 >= PENDING_BYTES) {
            self.count_dropped_cards(board, 1);
            return;
        }
        let queue = self.pending_cards.entry(board).or_default();
        if !queue.0.iter().any(|(existing, _)| id == *existing) {
            queue.0.push_back((id, card));
            let dropped = queue.trim();
            self.count_dropped_cards(board, dropped);
        }
    }

    pub(crate) fn reject_card(&mut self, board: BoardId, id: EventId) {
        let queue = self.rejected_cards.entry(board).or_default();
        if !queue.contains(&id) {
            queue.push_back(id);
            let dropped = queue.len().saturating_sub(REJECTED_COUNT);
            queue.drain(..dropped);
            self.count_dropped_cards(board, dropped);
        }
    }

    fn count_dropped_cards(&mut self, board: BoardId, dropped: usize) {
        if dropped != 0 {
            let count = self.dropped_cards.entry(board).or_default();
            *count = count.saturating_add(dropped);
        }
    }

    pub(crate) fn trim_card_queues(&mut self, catalog: BoardId) {
        // Older versions recorded rejected IDs without recording their boards.
        let legacy = std::mem::take(&mut self.invalid_board_cards);
        self.count_dropped_cards(catalog, legacy.len());
        let mut dropped = Vec::new();
        for (&board, queue) in &mut self.pending_cards {
            dropped.push((board, queue.trim()));
        }
        for (&board, people) in &mut self.people {
            let count = people
                .values_mut()
                .map(crate::BoardPerson::trim_record)
                .sum();
            dropped.push((board, count));
        }
        for (board, count) in dropped {
            self.count_dropped_cards(board, count);
        }
    }
}

const PREFIX: &[u8] = b"pepino-board-people\0";

#[derive(Deserialize, Serialize)]
pub enum BoardCards {
    Introduce(BoardAdmission),
    Update(IdentityCard),
    Membership(crate::MembershipChange),
    Leave(crate::SignedBoardLeave),
    ReaderRoute {
        channel: BoardId,
        person: crate::PersonKey,
        incarnation: Option<EventId>,
    },
}

#[derive(Serialize, Deserialize)]
struct Message<T> {
    v: u32,
    cards: T,
}

impl BoardCards {
    pub fn encode(&self) -> Result<Vec<u8>, AppError> {
        let mut out = PREFIX.to_vec();
        out.extend(WireFormat::Cbor.encode(&Message {
            v: crate::PROTOCOL_VERSION,
            cards: self,
        })?);
        Ok(out)
    }

    pub fn is_board_card(bytes: &[u8]) -> bool {
        bytes.starts_with(PREFIX)
    }

    pub fn decode(bytes: &[u8]) -> Result<Option<Self>, AppError> {
        let Some(body) = bytes.strip_prefix(PREFIX) else {
            return Ok(None);
        };
        let message = WireFormat::Cbor.decode::<Message<Self>>(body)?;
        Ok(Some(message.cards))
    }
}

impl App {
    pub async fn receive_board_cards(
        &self,
        board: BoardId,
        signer: DeviceId,
        creator: Option<DeviceId>,
        cards: BoardCards,
    ) -> Result<(), AppError> {
        let mut digest = blake3::Hasher::new_derive_key("Pepino rejected board card v1");
        digest
            .update(board.as_bytes())
            .update(signer.as_bytes())
            .update(&cards.encode()?);
        let id = EventId::from_bytes(*digest.finalize().as_bytes());
        match cards {
            BoardCards::Introduce(admission) if let Some(creator) = creator => {
                match self
                    .record_board_admission(board, signer, creator, admission)
                    .await
                {
                    Ok(()) => return Ok(()),
                    Err(AppError::InvalidInput { .. }) => (),
                    Err(error) => return Err(error),
                }
            }
            BoardCards::Introduce(_)
            | BoardCards::Membership(_)
            | BoardCards::ReaderRoute { .. } => (),
            BoardCards::Leave(request) => {
                return self
                    .record_board_leave(board, request)
                    .await
                    .or_else(|error| {
                        if matches!(error, AppError::InvalidInput { .. }) {
                            Ok(())
                        } else {
                            Err(error)
                        }
                    });
            }
            BoardCards::Update(card) => {
                return self.update_card(board, id, card).await;
            }
        }
        self.update_local(move |local| {
            local.catalog.reject_card(board, id);
        })
        .await
    }

    async fn update_card(
        &self,
        board: BoardId,
        id: EventId,
        card: IdentityCard,
    ) -> Result<(), AppError> {
        self.try_update_local_state(move |local| {
            let known_board = local.boards().iter().any(|entry| entry.id == board)
                && board != local.catalog_board();
            match local.catalog.people.get_mut(&board).and_then(|people| {
                people.get_mut(&crate::people_names::person_storage_key(card.key()))
            }) {
                Some(person) => {
                    if !person.merge(&card)? {
                        local.catalog.reject_card(board, id);
                    }
                }
                None if known_board => match card.verified_record() {
                    Ok(_) => local.catalog.queue_card(board, id, card),
                    Err(AppError::InvalidInput { .. }) => local.catalog.reject_card(board, id),
                    Err(error) => return Err(error),
                },
                None => (),
            }
            Ok(())
        })
        .await?;
        Ok(())
    }
}

impl LocalState {
    pub fn dropped_board_cards(&self) -> usize {
        self.catalog
            .dropped_cards
            .values()
            .copied()
            .fold(0, usize::saturating_add)
    }

    pub fn rejected_board_cards(&self) -> usize {
        self.catalog
            .rejected_cards
            .values()
            .map(VecDeque::len)
            .sum::<usize>()
            + self
                .catalog
                .people
                .values()
                .flat_map(|people| people.values())
                .map(crate::BoardPerson::rejected_entries)
                .sum::<usize>()
    }
}

#[cfg(test)]
#[tokio::test]
async fn card_updates_cannot_introduce_people() {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    let keys = app.identity_credentials().unwrap();
    let local = app
        .reconcile_identity(keys.device_id(), &keys)
        .await
        .unwrap();
    let id = BoardId::from_bytes([82; 32]);
    let update = BoardCards::Update(local.identity_card().unwrap())
        .encode()
        .unwrap();
    assert!(BoardCards::is_board_card(&update));
    assert!(BoardCards::decode(b"task event").unwrap().is_none());
    app.receive_board_cards(
        id,
        local.device_id,
        None,
        BoardCards::decode(&update).unwrap().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        app.fresh_local_state()
            .await
            .unwrap()
            .board_people(id)
            .count(),
        0
    );
}

#[cfg(test)]
#[tokio::test]
async fn repeated_unauthorized_introductions_count_once() {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    let keys = app.identity_credentials().unwrap();
    let local = app
        .reconcile_identity(keys.device_id(), &keys)
        .await
        .unwrap();
    let card = local.identity_card().unwrap();
    let id = BoardId::from_bytes([87; 32]);
    let stranger = DeviceId::from_bytes([88; 32]);
    let introduction = BoardAdmission {
        owner: card.clone(),
        owner_device: local.device_id,
        invitee: card,
        device: stranger,
        at: chrono::DateTime::from_timestamp(1795145000, 0).unwrap(),
    };
    for _ in 0..2 {
        app.receive_board_cards(
            id,
            stranger,
            None,
            BoardCards::Introduce(introduction.clone()),
        )
        .await
        .unwrap();
    }
    let after = app.fresh_local_state().await.unwrap();
    assert_eq!(after.rejected_board_cards(), 1);
    assert_eq!(after.board_people(id).count(), 0);
}

#[cfg(test)]
#[tokio::test]
async fn board_card_queues_evict_oldest_and_respect_both_limits() {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    let keys = app.identity_credentials().unwrap();
    let local = app
        .reconcile_identity(keys.device_id(), &keys)
        .await
        .unwrap();
    let card = local.identity_card().unwrap();
    let mut catalog = crate::catalog::CatalogState::default();
    let board = BoardId::from_bytes([60; 32]);
    let other = BoardId::from_bytes([61; 32]);
    let id = |index: usize| {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&(index as u64).to_be_bytes());
        EventId::from_bytes(bytes)
    };
    catalog.queue_card(other, id(0), card.clone());
    for index in 0..=PENDING_COUNT {
        catalog.queue_card(board, id(index), card.clone());
    }
    let queue = &catalog.pending_cards[&board].0;
    assert_eq!(queue.len(), PENDING_COUNT);
    assert_eq!(queue.front().unwrap().0, id(1));
    assert_eq!(catalog.pending_cards[&other].0.len(), 1);
    let mut encoded = serde_json::to_value(&card).unwrap();
    let edit = encoded["edits"][0].clone();
    let entry_bytes = serde_json::to_vec(&edit).unwrap().len() + 1;
    let repeats = PENDING_BYTES / (3 * entry_bytes) + 1;
    encoded["edits"] = serde_json::Value::Array(vec![edit.clone(); repeats]);
    let medium: IdentityCard = serde_json::from_value(encoded.clone()).unwrap();
    for index in 1..=3 {
        catalog.queue_card(other, id(index), medium.clone());
    }
    assert_eq!(catalog.pending_cards[&other].0.front().unwrap().0, id(2));
    assert_eq!(catalog.dropped_cards[&other], 2);

    encoded["edits"] = serde_json::Value::Array(vec![edit; PENDING_BYTES / 128]);
    catalog.queue_card(
        board,
        id(PENDING_COUNT + 1),
        serde_json::from_value(encoded).unwrap(),
    );
    assert_eq!(catalog.pending_cards[&board].0.len(), PENDING_COUNT);
    for index in 0..=REJECTED_COUNT {
        catalog.reject_card(board, id(index));
    }
    assert_eq!(catalog.rejected_cards[&board].front(), Some(&id(1)));
    assert!(
        serde_json::to_vec(&catalog.rejected_cards[&board])
            .unwrap()
            .len()
            <= REJECTED_BYTES
    );
    assert_eq!(catalog.dropped_cards[&board], 3);
}
