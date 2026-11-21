use std::collections::{BTreeMap, BTreeSet};

use p2panda::Hash;
use pepino_core::{BoardId, DeviceId};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::{
    Error,
    relay_control::{RelayAnswer, RelayInstruction, RelayRevision},
};

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
    pool: SqlitePool,
}

impl Ledger {
    pub(super) async fn load(
        pool: SqlitePool,
        trusted: BTreeSet<DeviceId>,
        cap: usize,
    ) -> Result<Self, Error> {
        let records = load_records(&pool).await?;
        let rejected = sqlx::query_scalar::<_, i64>("SELECT rejected FROM pepino_relay_stats")
            .fetch_one(&pool)
            .await
            .map_err(Error::storage)?;
        Ok(Self {
            records,
            rejected: rejected as u64,
            trusted,
            cap,
            pool,
        })
    }

    pub(super) fn is_trusted(&self, id: DeviceId) -> bool {
        self.trusted.contains(&id)
    }

    pub(super) async fn reject(&mut self) -> Result<RelayAnswer, Error> {
        sqlx::query(
            "UPDATE pepino_relay_stats SET rejected = MIN(rejected + 1, 9223372036854775807)",
        )
        .execute(&self.pool)
        .await
        .map_err(Error::storage)?;
        self.rejected = (self.rejected + 1).min(i64::MAX as u64);
        Ok(RelayAnswer::NotTrusted)
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

    pub(super) async fn apply(
        &mut self,
        writer: DeviceId,
        instruction: RelayInstruction,
    ) -> Result<RelayAnswer, Error> {
        if !self.is_trusted(writer) {
            return self.reject().await;
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
            sqlx::query("INSERT OR REPLACE INTO pepino_relay_ledger VALUES (?, ?)")
                .bind(key)
                .bind(encode(&next)?)
                .execute(&self.pool)
                .await
                .map_err(Error::storage)?;
            self.records.insert(key.to_vec(), next);
        }
        Ok(answer)
    }
}

pub(super) async fn prepare(pool: &SqlitePool) -> Result<(), Error> {
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS pepino_relay_ledger (key BLOB PRIMARY KEY, record BLOB NOT NULL) STRICT;
         CREATE TABLE IF NOT EXISTS pepino_relay_stats (singleton INTEGER PRIMARY KEY CHECK (singleton = 1), rejected INTEGER NOT NULL) STRICT;
         INSERT OR IGNORE INTO pepino_relay_stats VALUES (1, 0);"
    ).execute(pool).await.map_err(Error::storage)?;
    let records = load_records(pool).await?;
    super::relay_purge::pending(pool, records).await
}

async fn load_records(pool: &SqlitePool) -> Result<BTreeMap<Vec<u8>, Record>, Error> {
    sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>("SELECT key, record FROM pepino_relay_ledger")
        .fetch_all(pool)
        .await
        .map_err(Error::storage)?
        .into_iter()
        .map(|(key, record)| {
            p2panda_core::cbor::decode_cbor_strict(record.as_slice())
                .map(|record| (key, record))
                .map_err(|_| Error::Storage("invalid relay ledger record".into()))
        })
        .collect()
}

pub(super) fn encode(record: &Record) -> Result<Vec<u8>, Error> {
    p2panda_core::cbor::encode_cbor(record)
        .map_err(|_| Error::Storage("could not encode relay ledger record".into()))
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

    #[tokio::test]
    async fn trust_and_capacity_gate_durable_admission() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::node::open_database(&dir.path().join("relay.sqlite"), true)
            .await
            .unwrap();
        p2panda_store::sqlite::run_pending_migrations(&pool)
            .await
            .unwrap();
        prepare(&pool).await.unwrap();
        let trusted = DeviceId::from_bytes([17; 32]);
        let stranger = DeviceId::from_bytes([27; 32]);
        let mut ledger = Ledger::load(pool.clone(), [trusted].into(), 1)
            .await
            .unwrap();
        let add = |byte| RelayInstruction {
            board: BoardId::from_bytes([byte; 32]),
            sequence: 1,
            carry: true,
        };
        assert_eq!(
            ledger.apply(stranger, add(57)).await.unwrap(),
            RelayAnswer::NotTrusted
        );
        assert!(ledger.records.is_empty());
        assert_eq!(
            ledger.apply(trusted, add(57)).await.unwrap(),
            RelayAnswer::Carrying
        );
        assert_eq!(
            ledger.apply(trusted, add(58)).await.unwrap(),
            RelayAnswer::AtCapacity
        );
        assert_eq!(
            ledger.apply(trusted, add(57)).await.unwrap(),
            RelayAnswer::Carrying
        );
        let restored = Ledger::load(pool.clone(), [trusted].into(), 1)
            .await
            .unwrap();
        assert_eq!(restored.counts(), [(trusted, 1)].into());
        assert_eq!(restored.rejected, 1);
        let empty = Ledger::load(pool, [].into(), 64).await.unwrap();
        assert!(!empty.is_trusted(trusted));
    }
}
