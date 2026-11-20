use pepino_core::{BoardId, DeviceId, EventId};
use serde::{Deserialize, Serialize};

use crate::{App, AppError, BoardAdmission, IdentityCard, LocalState, WireFormat};

const PREFIX: &[u8] = b"pepino-board-people\0";

#[derive(Deserialize, Serialize)]
pub enum BoardCards {
    Introduce(BoardAdmission),
    Update(IdentityCard),
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

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        WireFormat::Cbor
            .decode::<Message<Self>>(bytes.strip_prefix(PREFIX)?)
            .ok()
            .map(|message| message.cards)
    }
}

impl App {
    pub async fn receive_board_cards(
        &self,
        board: BoardId,
        signer: DeviceId,
        manager: bool,
        cards: BoardCards,
    ) -> Result<(), AppError> {
        let mut digest = blake3::Hasher::new_derive_key("Pepino rejected board card v1");
        digest
            .update(board.as_bytes())
            .update(signer.as_bytes())
            .update(&cards.encode()?);
        let id = EventId::from_bytes(*digest.finalize().as_bytes());
        match cards {
            BoardCards::Introduce(admission) if manager => {
                match self.record_board_admission(board, signer, admission).await {
                    Ok(()) => return Ok(()),
                    Err(AppError::InvalidInput { .. }) => (),
                    Err(error) => return Err(error),
                }
            }
            BoardCards::Introduce(_) => (),
            BoardCards::Update(card) => {
                return self.update_card(board, id, card).await;
            }
        }
        self.update_local(move |local| {
            local.catalog.invalid_board_cards.insert(id);
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
            match local
                .catalog
                .people
                .get_mut(&board)
                .and_then(|people| people.get_mut(&card.id))
            {
                Some(person) => {
                    if !person.merge(&card)? {
                        local.catalog.invalid_board_cards.insert(id);
                    }
                }
                None if known_board => {
                    local
                        .catalog
                        .pending_cards
                        .entry(board)
                        .or_default()
                        .insert(id, card);
                }
                None => (),
            }
            Ok(())
        })
        .await?;
        Ok(())
    }
}

impl LocalState {
    pub fn rejected_board_cards(&self) -> usize {
        self.catalog.invalid_board_cards.len()
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
    assert!(BoardCards::decode(b"task event").is_none());
    app.receive_board_cards(
        id,
        local.device_id,
        false,
        BoardCards::decode(&update).unwrap(),
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
            false,
            BoardCards::Introduce(introduction.clone()),
        )
        .await
        .unwrap();
    }
    let after = app.fresh_local_state().await.unwrap();
    assert_eq!(after.rejected_board_cards(), 1);
    assert_eq!(after.board_people(id).count(), 0);
}
