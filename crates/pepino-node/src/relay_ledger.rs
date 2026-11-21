use std::collections::{BTreeMap, BTreeSet};

use p2panda::Hash;
use pepino_core::{BoardId, DeviceId};
use serde::{Deserialize, Serialize};

use crate::relay_control::{RelayAnswer, RelayInstruction, RelayRevision};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub(super) struct Record {
    pub(super) revision: RelayRevision,
    pub(super) answer: RelayAnswer,
    pub(super) board: Option<BoardId>,
}

impl Record {
    fn next(
        previous: Option<Self>,
        writer: DeviceId,
        instruction: RelayInstruction,
        below_capacity: bool,
    ) -> (Option<Self>, RelayAnswer) {
        let revision = RelayRevision {
            sequence: instruction.sequence,
            writer,
        };
        if let Some(record) = previous {
            if revision < record.revision {
                return (None, RelayAnswer::Superseded);
            }
            if revision == record.revision {
                return (None, record.answer);
            }
        }
        let active = previous.is_some_and(|record| record.board.is_some());
        let (answer, board) = match (instruction.carry, active, below_capacity) {
            (true, true, _) | (true, false, true) => {
                (RelayAnswer::Carrying, Some(instruction.board))
            }
            (true, false, false) => (RelayAnswer::AtCapacity, None),
            (false, true, _) => (RelayAnswer::RemovalPendingRestart, Some(instruction.board)),
            (false, false, _) => (RelayAnswer::Removed, None),
        };
        (
            Some(Self {
                revision,
                answer,
                board,
            }),
            answer,
        )
    }
}

pub(super) struct Ledger {
    pub(super) records: BTreeMap<Vec<u8>, Record>,
    pub(super) rejected: u64,
    trusted: BTreeSet<DeviceId>,
    cap: usize,
}

impl Ledger {
    pub(super) fn new(trusted: BTreeSet<DeviceId>, cap: usize) -> Self {
        Self {
            records: BTreeMap::new(),
            rejected: 0,
            trusted,
            cap,
        }
    }

    pub(super) fn is_trusted(&self, id: DeviceId) -> bool {
        self.trusted.contains(&id)
    }

    pub(super) fn reject(&mut self) -> RelayAnswer {
        self.rejected = self.rejected.saturating_add(1);
        RelayAnswer::NotTrusted
    }

    pub(super) fn counts(&self) -> BTreeMap<DeviceId, usize> {
        let mut counts: BTreeMap<_, _> = self.trusted.iter().map(|&id| (id, 0)).collect();
        for record in self
            .records
            .values()
            .filter(|record| record.board.is_some())
        {
            *counts.entry(record.revision.writer).or_default() += 1;
        }
        counts
    }

    pub(super) fn apply(&mut self, writer: DeviceId, instruction: RelayInstruction) -> RelayAnswer {
        if !self.is_trusted(writer) {
            return self.reject();
        }
        let key = Hash::digest(instruction.board.as_bytes());
        let key = key.as_bytes().as_slice();
        let below_capacity = self
            .records
            .values()
            .filter(|record| record.board.is_some())
            .count()
            < self.cap;
        let (next, answer) = Record::next(
            self.records.get(key).copied(),
            writer,
            instruction,
            below_capacity,
        );
        if let Some(next) = next {
            self.records.insert(key.to_vec(), next);
        }
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn replays_in_any_order_converge_to_the_latest_revision(
            order in any::<[u8; 8]>(),
            latest_carries in any::<bool>(),
        ) {
            let board = BoardId::from_bytes([42; 32]);
            let authors = [DeviceId::from_bytes([61; 32]), DeviceId::from_bytes([73; 32])];
            let mut deliveries: Vec<_> = order.into_iter().enumerate().collect();
            deliveries.sort_by_key(|(_, value)| *value);
            let mut record = None;
            for (index, _) in deliveries {
                let carry = if index == 7 { latest_carries } else { index % 2 == 0 };
                let instruction = RelayInstruction { board, sequence: (index / 2) as u64, carry };
                let (next, _) = Record::next(record, authors[index % 2], instruction, true);
                record = next.or(record);
            }
            let record = record.unwrap();
            prop_assert_eq!(record.revision, RelayRevision { sequence: 3, writer: authors[1] });
            prop_assert_eq!(record.answer == RelayAnswer::Carrying, latest_carries);
            let old_add = RelayInstruction { board, sequence: 3, carry: true };
            prop_assert_eq!(Record::next(Some(record), authors[0], old_add, true), (None, RelayAnswer::Superseded));
        }
    }
}
