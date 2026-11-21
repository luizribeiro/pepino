use chrono::{DateTime, Utc};
use pepino_core::{BoardId, DeviceId, EventId};
use serde::{Deserialize, Serialize};

use crate::{App, AppError, BoardRole, LocalState, PersonKey};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipAction {
    Invited(BoardRole),
    RoleChanged(BoardRole),
    Removed,
    Left,
}

impl MembershipAction {
    pub fn role(self) -> Option<BoardRole> {
        match self {
            Self::Invited(role) | Self::RoleChanged(role) => Some(role),
            Self::Removed | Self::Left => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MembershipChange {
    pub person: PersonKey,
    pub action: MembershipAction,
    pub at: DateTime<Utc>,
    pub revision: u64,
    pub incarnation: Option<EventId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MembershipRecord {
    pub operation: EventId,
    pub actor: DeviceId,
    pub change: MembershipChange,
}

impl MembershipRecord {
    fn order(&self) -> (u64, bool, EventId) {
        (
            self.change.revision,
            self.change.action.role().is_none(),
            self.operation,
        )
    }
}

impl LocalState {
    pub fn membership_history(&self, board: BoardId) -> impl Iterator<Item = &MembershipRecord> {
        self.catalog
            .membership
            .get(&board)
            .into_iter()
            .flat_map(|records| records.values())
    }

    pub fn latest_membership(
        &self,
        board: BoardId,
        person: PersonKey,
    ) -> Option<&MembershipRecord> {
        let incarnation = self.membership_incarnation(board, person);
        self.membership_history(board)
            .filter(|record| {
                record.change.person == person
                    && match record.change.action {
                        MembershipAction::Invited(_) => Some(record.operation) == incarnation,
                        _ => record.change.incarnation == incarnation,
                    }
            })
            .max_by_key(|record| (record.change.action.role().is_none(), record.order()))
    }

    pub fn membership_incarnation(&self, board: BoardId, person: PersonKey) -> Option<EventId> {
        self.membership_history(board)
            .filter(|record| {
                record.change.person == person
                    && matches!(record.change.action, MembershipAction::Invited(_))
            })
            .max_by_key(|record| record.order())
            .map(|record| record.operation)
    }

    pub fn next_membership_revision(
        &self,
        board: BoardId,
        person: PersonKey,
    ) -> Result<u64, AppError> {
        self.latest_membership(board, person)
            .map_or(Some(1), |last| last.change.revision.checked_add(1))
            .ok_or_else(|| crate::app::invalid("person", "membership revision limit reached"))
    }
}

impl App {
    pub async fn record_membership(
        &self,
        board: BoardId,
        record: MembershipRecord,
    ) -> Result<(), AppError> {
        self.update_local(move |local| {
            if board != local.catalog_board()
                && local.boards().iter().any(|known| known.id == board)
            {
                local
                    .catalog
                    .membership
                    .entry(board)
                    .or_default()
                    .entry(record.operation)
                    .or_insert(record);
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[tokio::test]
    async fn membership_history_resolves_concurrent_actions_regardless_of_delivery_order() {
        let dir = tempfile::tempdir().unwrap();
        let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
        let base = app.local_state();
        let board = BoardId::from_bytes([78; 32]);
        let person = base.own_person_key();
        let at = DateTime::from_timestamp(1795145000, 0).unwrap();
        let actor = DeviceId::from_bytes([59; 32]);
        let records = [
            (8, 1, MembershipAction::Invited(BoardRole::Read)),
            (9, 2, MembershipAction::RoleChanged(BoardRole::Write)),
            (7, 2, MembershipAction::Removed),
            (4, 3, MembershipAction::Invited(BoardRole::Read)),
        ]
        .map(|(id, revision, action)| MembershipRecord {
            operation: EventId::from_bytes([id; 32]),
            actor,
            change: MembershipChange {
                person,
                action,
                at,
                revision,
                incarnation: (revision == 2).then_some(EventId::from_bytes([8; 32])),
            },
        });
        proptest!(|(priorities in any::<[u8; 4]>())| {
            let mut local = base.clone();
            let mut order = [0, 1, 2, 3];
            order.sort_by_key(|&index| priorities[index]);
            for index in order {
                let record = &records[index];
                local.catalog.membership.entry(board).or_default().insert(record.operation, record.clone());
            }
            prop_assert_eq!(local.latest_membership(board, person), Some(&records[3]));
            prop_assert_eq!(local.membership_history(board).count(), 4);
            local.catalog.membership.get_mut(&board).unwrap().remove(&records[3].operation);
            prop_assert_eq!(local.latest_membership(board, person), Some(&records[2]));
        });
    }
}
