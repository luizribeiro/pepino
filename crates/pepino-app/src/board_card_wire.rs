use pepino_core::{BoardId, DeviceId};
use serde::{Deserialize, Serialize};

use crate::{App, AppError, BoardAdmission, IdentityCard, WireFormat};

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
        match cards {
            BoardCards::Introduce(admission) if manager => {
                self.record_board_admission(board, signer, admission).await
            }
            BoardCards::Introduce(_) => Ok(()),
            BoardCards::Update(card) => {
                self.try_update_local_state(move |local| {
                    if let Some(person) = local
                        .catalog
                        .people
                        .get_mut(&board)
                        .and_then(|people| people.get_mut(&card.id))
                    {
                        person.merge(&card)?;
                    }
                    Ok(())
                })
                .await?;
                Ok(())
            }
        }
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
