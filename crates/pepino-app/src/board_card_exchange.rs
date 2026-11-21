use std::collections::BTreeMap;

use pepino_core::{BoardId, EventId, IdentityId};
use serde::{Deserialize, Serialize};

use crate::{App, AppError, LocalState, app::invalid};

const MAX_CARD_EXCHANGES_PER_BOARD: usize = 128;

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

    pub(crate) fn card_exchange_has_room(&self, board: BoardId, channel: BoardId) -> bool {
        self.card_exchanges().contains_key(&channel)
            || self
                .card_exchanges()
                .values()
                .filter(|route| route.board == board)
                .count()
                < MAX_CARD_EXCHANGES_PER_BOARD
    }

    pub(crate) fn cap_card_exchanges(&mut self) {
        let mut counts = BTreeMap::<_, usize>::new();
        self.catalog.card_exchanges.retain(|_, route| {
            let count = counts.entry(route.board).or_default();
            *count += 1;
            *count <= MAX_CARD_EXCHANGES_PER_BOARD
        });
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
        if !self.card_exchange_has_room(exchange.board, channel) {
            return Err(invalid(
                "grant",
                "too many identity update channels for this board",
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

#[cfg(test)]
#[tokio::test]
async fn the_card_exchange_limit_is_per_board_and_allows_existing_routes_to_update() {
    let directory = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(directory.path(), 0)
        .await
        .unwrap();
    let mut local = app.local_state().clone();
    let channel = |n| {
        let mut bytes = [n; 32];
        bytes[0] = 219;
        BoardId::from_bytes(bytes)
    };
    let mut exchange = CardExchange {
        board: channel(245),
        root: None,
        incarnation: None,
        person: IdentityId::from_bytes([73; 32]),
    };
    for i in 0..MAX_CARD_EXCHANGES_PER_BOARD {
        local.add_card_exchange(channel(i as u8), exchange).unwrap();
    }
    let overflow = channel(MAX_CARD_EXCHANGES_PER_BOARD as u8);
    assert!(local.add_card_exchange(overflow, exchange).is_err());
    local.catalog.card_exchanges.insert(overflow, exchange);
    local.cap_card_exchanges();
    assert!(!local.card_exchanges().contains_key(&overflow));

    exchange.root = Some(local.device_id);
    local.add_card_exchange(channel(0), exchange).unwrap();
    assert_eq!(local.card_exchanges()[&channel(0)].root, exchange.root);

    exchange.board = channel(246);
    assert!(local.add_card_exchange(overflow, exchange).is_ok());
}
