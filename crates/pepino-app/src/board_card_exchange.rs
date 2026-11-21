use std::collections::BTreeMap;

use pepino_core::{BoardId, EventId, IdentityId};
use serde::{Deserialize, Serialize};

use crate::{App, AppError, LocalState, app::invalid};

const MAX_ROUTES_PER_PERSON: usize = 4;

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
            && self.card_exchange_person_is_current(route.board, person)
            && self
                .membership_incarnation(route.board, person)
                .is_none_or(|incarnation| route.incarnation == Some(incarnation))
    }

    pub(crate) fn device_update_incarnation(
        &self,
        board: BoardId,
        person: crate::PersonKey,
    ) -> Option<EventId> {
        self.membership_incarnation(board, person).or_else(|| {
            self.catalog
                .joined_shares
                .get(&board)
                .and_then(|share| share.card_exchange)
                .and_then(|channel| self.card_exchanges().get(&channel))
                .filter(|route| self.is_current_exchange(route, person))
                .or_else(|| {
                    self.card_exchanges().values().find(|route| {
                        route.board == board && self.is_current_exchange(route, person)
                    })
                })
                .and_then(|route| route.incarnation)
        })
    }

    pub(crate) fn matches_device_update_incarnation(
        &self,
        board: BoardId,
        person: crate::PersonKey,
        incarnation: Option<EventId>,
    ) -> bool {
        if !self.card_exchange_person_is_current(board, person) {
            return false;
        }
        if let Some(effective) = self.membership_incarnation(board, person) {
            return incarnation == Some(effective);
        }
        // A surviving manager can verify a route even if it refused the original invitation.
        let mut routes = self
            .card_exchanges()
            .values()
            .filter(|route| route.board == board && route.matches(person))
            .peekable();
        if routes.peek().is_none() {
            return incarnation.is_none();
        }
        routes.any(|route| route.incarnation == incarnation)
    }

    pub fn card_exchanges(&self) -> &BTreeMap<BoardId, CardExchange> {
        &self.catalog.card_exchanges
    }

    fn card_exchange_person_is_current(&self, board: BoardId, person: crate::PersonKey) -> bool {
        if let Some(record) = self.latest_membership(board, person) {
            return record.change.action.role().is_some();
        }
        self.card_exchange_person_is_known(board, person)
    }

    fn card_exchange_person_is_known(&self, board: BoardId, person: crate::PersonKey) -> bool {
        let own = self.own_person_key() == person;
        let known = own || self.board_people(board).any(|known| known.key() == person);
        known
            && self.observed_members(board).is_none_or(|members| {
                if own
                    && self.waiting_for_access(board)
                    && !self.catalog.observed_board_removals.contains(&board)
                {
                    return true;
                }
                !self.membership_devices(board, person).is_disjoint(members)
                    || (own && members.contains(&self.device_id))
            })
    }

    fn retained_card_exchange_person(&self, route: &CardExchange) -> Option<crate::PersonKey> {
        let board = route.board;
        if board == self.catalog_board() || !self.boards().iter().any(|known| known.id == board) {
            return None;
        }
        let own = self.own_person_key();
        let person = route
            .root
            .map(|root| (route.person, root))
            .or_else(|| route.matches(own).then_some(own))
            .or_else(|| {
                self.board_people(board)
                    .find(|known| route.matches(known.key()))
                    .map(crate::BoardPerson::key)
            })?;
        if person == own && self.needs_readmission_replay(board) {
            return Some(person);
        }
        if let Some(record) = self.latest_membership(board, person) {
            if record.change.action.role().is_none() {
                // Keep the route available while a manager removes channel access.
                return (!record.applied).then_some(person);
            }
            let current = route.incarnation.is_none_or(|incarnation| {
                self.membership_incarnation(board, person)
                    .is_none_or(|current| current == incarnation)
            });
            return current.then_some(person);
        }
        self.card_exchange_person_is_known(board, person)
            .then_some(person)
    }

    pub(crate) fn prune_card_exchanges(&mut self) {
        let mut routes = std::mem::take(&mut self.catalog.card_exchanges);
        let order = std::mem::take(&mut self.catalog.card_exchange_order);
        // Stores without receipt order fall back to channel IDs.
        let legacy_order: Vec<_> = routes.keys().copied().collect();
        let mut counts = BTreeMap::new();
        for channel in order
            .into_iter()
            .rev()
            .chain(legacy_order.into_iter().rev())
        {
            let Some(route) = routes.remove(&channel) else {
                continue;
            };
            let Some(person) = self.retained_card_exchange_person(&route) else {
                continue;
            };
            let count = counts.entry((route.board, person)).or_insert(0);
            if *count < MAX_ROUTES_PER_PERSON {
                *count += 1;
                self.catalog.card_exchanges.insert(channel, route);
                self.catalog.card_exchange_order.push(channel);
            }
        }
        self.catalog.card_exchange_order.reverse();
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
        if self.retained_card_exchange_person(&exchange).is_none() {
            return Ok(());
        }
        if let Some(current) = self.catalog.card_exchanges.get_mut(&channel) {
            current.root = current.root.or(exchange.root);
            current.incarnation = current.incarnation.or(exchange.incarnation);
        } else {
            self.catalog.card_exchanges.insert(channel, exchange);
            self.catalog.card_exchange_order.push(channel);
        }
        self.prune_card_exchanges();
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
        person: app.local_state().identity_id,
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
async fn only_current_members_and_pending_invitees_retain_channels() {
    let directory = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(directory.path(), 0)
        .await
        .unwrap();
    let board = BoardId::from_bytes([211; 32]);
    app.register_created_board(board, "Garden workshop")
        .await
        .unwrap();
    let mut local = app.fresh_local_state().await.unwrap();
    let invitation = |n: u16| {
        let mut bytes = [0; 32];
        bytes[..2].copy_from_slice(&n.to_be_bytes());
        EventId::from_bytes(bytes)
    };
    for index in 0..200u16 {
        let operation = invitation(index);
        let person = (
            IdentityId::from_bytes(*operation.as_bytes()),
            pepino_core::DeviceId::from_bytes(*operation.as_bytes()),
        );
        let route = CardExchange {
            board,
            person: person.0,
            root: Some(person.1),
            incarnation: Some(operation),
        };
        let channel = BoardId::from_bytes(*operation.as_bytes());
        local.catalog.card_exchanges.insert(channel, route);
        local.prune_card_exchanges();
        assert_eq!(local.card_exchanges().len(), usize::from(index));
        local.catalog.membership.entry(board).or_default().insert(
            operation,
            crate::MembershipRecord {
                operation,
                actor: local.device_id,
                change: crate::MembershipChange {
                    person,
                    action: crate::MembershipAction::Invited(crate::BoardRole::Read),
                    at: chrono::DateTime::UNIX_EPOCH,
                    revision: 1,
                    incarnation: None,
                    parents: Default::default(),
                },
                refusal: None,
                applied: false,
                relayed: false,
            },
        );
        local.add_card_exchange(channel, route).unwrap();
        assert_eq!(local.card_exchanges().len(), usize::from(index) + 1);
    }
    let operation = invitation(200);
    let person = (
        IdentityId::from_bytes([211; 32]),
        pepino_core::DeviceId::from_bytes([13; 32]),
    );
    local
        .add_card_exchange(
            BoardId::from_bytes(*operation.as_bytes()),
            CardExchange {
                board,
                person: person.0,
                root: Some(person.1),
                incarnation: Some(operation),
            },
        )
        .unwrap();
    assert_eq!(local.card_exchanges().len(), 200);
    let channel = BoardId::from_bytes([198; 32]);
    local.catalog.card_exchanges.insert(
        channel,
        CardExchange {
            board,
            person: person.0,
            root: Some(person.1),
            incarnation: None,
        },
    );
    local.replace(directory.path()).unwrap();
    assert!(
        !app.fresh_local_state()
            .await
            .unwrap()
            .card_exchanges()
            .contains_key(&channel)
    );
}
