use std::collections::BTreeMap;

use pepino_core::{BoardId, IdentityId};
use serde::{Deserialize, Serialize};

use crate::{App, AppError, LocalState, app::invalid};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CardExchange {
    pub board: BoardId,
    pub person: IdentityId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<pepino_core::DeviceId>,
}

impl LocalState {
    pub fn card_exchanges(&self) -> &BTreeMap<BoardId, CardExchange> {
        &self.catalog.card_exchanges
    }

    pub(crate) fn add_card_exchange(
        &mut self,
        channel: BoardId,
        exchange: CardExchange,
    ) -> Result<(), AppError> {
        if self.boards().iter().any(|board| board.id == channel)
            || self
                .card_exchanges()
                .get(&channel)
                .is_some_and(|old| old != &exchange)
        {
            return Err(invalid(
                "grant",
                "the identity update channel is already in use",
            ));
        }
        self.catalog.card_exchanges.insert(channel, exchange);
        Ok(())
    }
}

impl App {
    pub async fn register_card_exchange(
        &self,
        channel: BoardId,
        exchange: CardExchange,
    ) -> Result<(), AppError> {
        self.try_update_local_state(move |local| local.add_card_exchange(channel, exchange))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
#[tokio::test]
async fn card_exchanges_are_retained_without_becoming_visible_boards() {
    let directory = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(directory.path(), 0)
        .await
        .unwrap();
    let board = BoardId::from_bytes([71; 32]);
    let channel = BoardId::from_bytes([72; 32]);
    let exchange = CardExchange {
        board,
        root: None,
        person: IdentityId::from_bytes([73; 32]),
    };
    app.register_created_board(board, "House").await.unwrap();
    app.register_card_exchange(channel, exchange).await.unwrap();
    let saved = app.fresh_local_state().await.unwrap();
    assert_eq!(saved.card_exchanges()[&channel], exchange);
    assert!(saved.boards().iter().all(|entry| entry.id != channel));
    assert!(app.register_card_exchange(board, exchange).await.is_err());
}
