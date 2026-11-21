use std::collections::BTreeMap;

use pepino_core::{BoardId, EventId, IdentityId};
use serde::{Deserialize, Serialize};

use crate::{App, AppError, LocalState, app::invalid};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CardExchange {
    pub board: BoardId,
    pub person: IdentityId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<pepino_core::DeviceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incarnation: Option<EventId>,
}

impl CardExchange {
    pub fn matches(&self, person: crate::PersonKey) -> bool {
        self.person == person.0 && self.root.is_none_or(|root| root == person.1)
    }
}

impl LocalState {
    pub fn is_current_exchange(&self, route: &CardExchange, person: crate::PersonKey) -> bool {
        route.matches(person)
            && route.incarnation == self.membership_incarnation(route.board, person)
    }

    pub fn card_exchanges(&self) -> &BTreeMap<BoardId, CardExchange> {
        &self.catalog.card_exchanges
    }

    pub(crate) fn add_card_exchange(
        &mut self,
        channel: BoardId,
        exchange: CardExchange,
    ) -> Result<(), AppError> {
        if self.catalog.forgotten_boards.contains(&exchange.board) {
            return Ok(());
        }
        if self.boards().iter().any(|board| board.id == channel)
            || self.card_exchanges().get(&channel).is_some_and(|old| {
                old.board != exchange.board
                    || old.person != exchange.person
                    || (old.root.is_some() && exchange.root.is_some() && old.root != exchange.root)
                    || (old.incarnation.is_some()
                        && exchange.incarnation.is_some()
                        && old.incarnation != exchange.incarnation)
            })
        {
            return Err(invalid(
                "grant",
                "the identity update channel is already in use",
            ));
        }
        let current = self
            .catalog
            .card_exchanges
            .entry(channel)
            .or_insert(exchange);
        current.root = current.root.or(exchange.root);
        current.incarnation = current.incarnation.or(exchange.incarnation);
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
        incarnation: None,
        person: IdentityId::from_bytes([73; 32]),
    };
    app.register_created_board(board, "House").await.unwrap();
    app.register_card_exchange(channel, exchange).await.unwrap();
    let saved = app.fresh_local_state().await.unwrap();
    assert_eq!(saved.card_exchanges()[&channel], exchange);
    assert!(saved.boards().iter().all(|entry| entry.id != channel));
    assert!(app.register_card_exchange(board, exchange).await.is_err());
}
