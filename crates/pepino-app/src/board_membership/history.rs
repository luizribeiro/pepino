use std::collections::{BTreeMap, BTreeSet};

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
    // Retained for older peers. The signed parent links determine causal order.
    pub revision: u64,
    pub incarnation: Option<EventId>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub parents: BTreeSet<EventId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MembershipRecord {
    pub operation: EventId,
    pub actor: DeviceId,
    pub change: MembershipChange,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<MembershipRefusal>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub applied: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub relayed: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipRefusal {
    AuthorNoLongerManager,
    LastManager,
}

impl std::fmt::Display for MembershipRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::AuthorNoLongerManager => "the author no longer has the manage role",
            Self::LastManager => "the board would lose its last manager",
        })
    }
}

impl MembershipRecord {
    fn is_removal(&self) -> bool {
        self.change.action.role().is_none()
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

    fn person_membership(
        &self,
        board: BoardId,
        person: PersonKey,
    ) -> BTreeMap<EventId, &MembershipRecord> {
        self.membership_history(board)
            .filter(|record| record.change.person == person && record.refusal.is_none())
            .map(|record| (record.operation, record))
            .collect()
    }

    pub fn membership_parents(&self, board: BoardId, person: PersonKey) -> BTreeSet<EventId> {
        let records = self.person_membership(board, person);
        let mut parents: BTreeSet<_> = records.keys().copied().collect();
        for record in records.values() {
            parents.retain(|parent| !record.change.parents.contains(parent));
        }
        parents
    }

    pub fn latest_membership(
        &self,
        board: BoardId,
        person: PersonKey,
    ) -> Option<&MembershipRecord> {
        let records = self.person_membership(board, person);
        if let Some(invitation) = current_invitation(&records) {
            return latest_matching(&records, |record| {
                record.operation == invitation.operation
                    || (matches!(record.change.action, MembershipAction::RoleChanged(_))
                        && record.change.incarnation == Some(invitation.operation))
            });
        }
        latest_matching(&records, MembershipRecord::is_removal).or_else(|| {
            latest_matching(&records, |record| {
                matches!(record.change.action, MembershipAction::RoleChanged(_))
                    && record.change.incarnation.is_none()
            })
        })
    }

    pub fn membership_incarnation(&self, board: BoardId, person: PersonKey) -> Option<EventId> {
        current_invitation(&self.person_membership(board, person)).map(|record| record.operation)
    }

    pub fn membership_superseded(&self, board: BoardId, record: &MembershipRecord) -> bool {
        let records = self.person_membership(board, record.change.person);
        let concurrent_removal = |removal: &&MembershipRecord| {
            removal.is_removal() && !follows(&records, record, removal.operation)
        };
        match record.change.action {
            MembershipAction::Invited(_) => records.values().any(concurrent_removal),
            MembershipAction::RoleChanged(_) => records.values().any(|removal| {
                concurrent_removal(removal) && !follows(&records, removal, record.operation)
            }),
            MembershipAction::Left | MembershipAction::Removed => false,
        }
    }

    pub fn next_membership_revision(&self, board: BoardId, person: PersonKey) -> u64 {
        // Older peers order records by this field. A new record follows the known heads.
        self.person_membership(board, person).len() as u64 + 1
    }
}

fn current_invitation<'a>(
    records: &BTreeMap<EventId, &'a MembershipRecord>,
) -> Option<&'a MembershipRecord> {
    latest_matching(records, |record| {
        matches!(record.change.action, MembershipAction::Invited(_))
            && records
                .values()
                .filter(|other| other.is_removal())
                .all(|removal| follows(records, record, removal.operation))
    })
}

fn latest_matching<'a>(
    records: &BTreeMap<EventId, &'a MembershipRecord>,
    matches: impl Fn(&MembershipRecord) -> bool,
) -> Option<&'a MembershipRecord> {
    let candidates: Vec<_> = records
        .values()
        .copied()
        .filter(|record| matches(record))
        .collect();
    let mut superseded = BTreeSet::new();
    for record in &candidates {
        superseded.extend(
            ancestors(records, record)
                .into_iter()
                .filter(|id| *id != record.operation),
        );
    }
    candidates
        .into_iter()
        .rev()
        .find(|record| !superseded.contains(&record.operation))
}

fn follows(
    records: &BTreeMap<EventId, &MembershipRecord>,
    record: &MembershipRecord,
    ancestor: EventId,
) -> bool {
    ancestors(records, record).contains(&ancestor)
}

fn ancestors(
    records: &BTreeMap<EventId, &MembershipRecord>,
    record: &MembershipRecord,
) -> BTreeSet<EventId> {
    let mut remaining: Vec<_> = record.change.parents.iter().copied().collect();
    let mut visited = BTreeSet::new();
    while let Some(parent) = remaining.pop() {
        if visited.insert(parent)
            && let Some(previous) = records.get(&parent)
        {
            remaining.extend(previous.change.parents.iter().copied());
        }
    }
    visited
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
                    .and_modify(|existing| {
                        if existing.relayed {
                            *existing = record.clone();
                        }
                    })
                    .or_insert(record);
            }
        })
        .await
    }
    pub async fn mark_membership_result(
        &self,
        board: BoardId,
        operation: EventId,
        result: Result<(), MembershipRefusal>,
    ) -> Result<(), AppError> {
        self.update_local(move |local| {
            if let Some(record) = local
                .catalog
                .membership
                .get_mut(&board)
                .and_then(|records| records.get_mut(&operation))
                && !record.applied
                && record.refusal.is_none()
            {
                record.applied = result.is_ok();
                record.refusal = result.err();
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn delivery_orders() -> Vec<[usize; 4]> {
        let mut orders = vec![[0, 1, 2, 3]];
        for first in 0..3 {
            let mut next = Vec::new();
            for order in orders {
                for position in first..4 {
                    let mut order = order;
                    order.swap(first, position);
                    next.push(order);
                }
            }
            orders = next;
        }
        orders
    }

    #[tokio::test]
    async fn refused_removals_do_not_change_membership_or_parents() {
        let dir = tempfile::tempdir().unwrap();
        let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
        let board = BoardId::from_bytes([78; 32]);
        app.register_created_board(board, "Garden workshop")
            .await
            .unwrap();
        let person = app.local_state().own_person_key();
        let invitation = EventId::from_bytes([9; 32]);
        let active = MembershipRecord {
            operation: invitation,
            actor: app.local_state().device_id,
            refusal: None,
            applied: true,
            relayed: false,
            change: MembershipChange {
                person,
                action: MembershipAction::Invited(BoardRole::Write),
                at: DateTime::from_timestamp(1795145000, 0).unwrap(),
                revision: 1,
                incarnation: None,
                parents: BTreeSet::new(),
            },
        };
        app.record_membership(board, active.clone()).await.unwrap();
        for (operation, refusal) in [
            (EventId::from_bytes([2; 32]), MembershipRefusal::LastManager),
            (
                EventId::from_bytes([3; 32]),
                MembershipRefusal::AuthorNoLongerManager,
            ),
        ] {
            app.record_membership(
                board,
                MembershipRecord {
                    operation,
                    actor: DeviceId::from_bytes([61; 32]),
                    applied: false,
                    refusal: Some(refusal),
                    change: MembershipChange {
                        action: MembershipAction::Removed,
                        parents: [invitation].into(),
                        ..active.change.clone()
                    },
                    ..active.clone()
                },
            )
            .await
            .unwrap();
        }
        let local = app.fresh_local_state().await.unwrap();
        assert_eq!(local.membership_history(board).count(), 3);
        assert_eq!(local.latest_membership(board, person), Some(&active));
        assert_eq!(local.membership_parents(board, person), [invitation].into());
        assert_eq!(
            local.membership_incarnation(board, person),
            Some(invitation)
        );
    }

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
            refusal: None,
            applied: false,
            relayed: false,
            change: MembershipChange {
                person,
                action,
                at,
                revision,
                incarnation: (revision == 2).then_some(EventId::from_bytes([8; 32])),
                parents: (revision == 2)
                    .then_some(EventId::from_bytes([8; 32]))
                    .into_iter()
                    .collect(),
            },
        });
        let removal = records[2].operation;
        proptest!(|(saw_removal in any::<bool>(), leave in any::<bool>(), advisory_revisions in any::<[u64; 4]>())| {
            for manager_count in 2..=3 {
                let mut records = records.clone();
                for (index, record) in records.iter_mut().enumerate() {
                    record.actor = DeviceId::from_bytes([59 + (index % manager_count) as u8; 32]);
                    record.change.revision = if index == 0 { u64::MAX } else { advisory_revisions[index] };
                }
                if leave {
                    records[2].change.action = MembershipAction::Left;
                }
                if saw_removal {
                    records[3].change.parents.insert(removal);
                }
                for order in delivery_orders() {
                    let mut local = base.clone();
                    for index in order {
                        let record = &records[index];
                        local.catalog.membership.entry(board).or_default().insert(record.operation, record.clone());
                    }
                    let latest = if saw_removal { &records[3] } else { &records[2] };
                    prop_assert_eq!(local.latest_membership(board, person), Some(latest));
                    prop_assert_eq!(local.membership_superseded(board, &records[3]), !saw_removal);
                    prop_assert!(local.membership_superseded(board, &records[1]));
                    prop_assert_eq!(local.membership_history(board).count(), 4);
                    prop_assert_eq!(local.next_membership_revision(board, person), 5);
                    local.catalog.membership.get_mut(&board).unwrap().remove(&records[3].operation);
                    prop_assert_eq!(local.latest_membership(board, person), Some(&records[2]));
                }
            }
        });
    }
}
