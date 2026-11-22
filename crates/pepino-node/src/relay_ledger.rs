use std::collections::{BTreeMap, BTreeSet};

use p2panda::Hash;
use pepino_core::{BoardId, DeviceId};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::{
    Error,
    relay_control::{RelayAnswer, RelayInstruction, RelayRevision},
};

pub const CLOCK_TOLERANCE_MS: u64 = 10 * 60 * 1000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub(super) struct Request {
    pub(super) sequence: u64,
    pub(super) answer: RelayAnswer,
}

/// What the relay holds for one board: the board while a copy is stored, and the latest
/// instruction of every device that asked for it.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub(super) struct Record {
    pub(super) board: Option<BoardId>,
    pub(super) requests: BTreeMap<DeviceId, Request>,
}

impl Record {
    fn requesters(&self) -> impl Iterator<Item = (DeviceId, Request)> {
        self.requests
            .iter()
            .filter(|(_, request)| request.answer == RelayAnswer::Carrying)
            .map(|(&device, &request)| (device, request))
    }

    pub(super) fn removal_pending(&self) -> bool {
        self.board.is_some() && self.requesters().next().is_none()
    }

    /// Ends the requests of the leaving devices. The copy is only removed with its last
    /// requester, and a device that leaves others behind is told that its own request is gone.
    fn withdraw(&mut self, leaving: &[(DeviceId, u64)]) -> RelayAnswer {
        for (device, _) in leaving {
            self.requests.remove(device);
        }
        let answer = if self.removal_pending() {
            RelayAnswer::RemovalPendingRestart
        } else {
            RelayAnswer::Removed
        };
        let ended = leaving
            .iter()
            .map(|&(device, sequence)| (device, Request { sequence, answer }));
        self.requests.extend(ended);
        answer
    }

    fn settle_pending_removals(&mut self) {
        for request in self.requests.values_mut() {
            if request.answer == RelayAnswer::RemovalPendingRestart {
                request.answer = RelayAnswer::Removed;
            }
        }
    }

    pub(super) fn purged(mut self) -> Self {
        self.board = None;
        self.settle_pending_removals();
        self
    }

    fn next(
        previous: Option<&Self>,
        writer: DeviceId,
        instruction: RelayInstruction,
        below_capacity: bool,
    ) -> (Option<Self>, RelayAnswer) {
        let request = previous.and_then(|record| record.requests.get(&writer));
        if let Some(request) = request {
            if instruction.sequence < request.sequence {
                return (None, RelayAnswer::Superseded);
            }
            if instruction.sequence == request.sequence {
                return (None, request.answer);
            }
        }
        let mut record = previous.cloned().unwrap_or_default();
        let sequence = instruction.sequence;
        let answer = if instruction.carry {
            if record.board.is_none() && !below_capacity {
                return (None, RelayAnswer::AtCapacity);
            }
            record.settle_pending_removals();
            record.board = Some(instruction.board);
            let answer = RelayAnswer::Carrying;
            record.requests.insert(writer, Request { sequence, answer });
            answer
        } else if request.is_none() {
            return (None, RelayAnswer::Removed);
        } else {
            record.withdraw(&[(writer, sequence)])
        };
        (Some(record), answer)
    }
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct SingleRequesterRecord {
    revision: RelayRevision,
    answer: RelayAnswer,
    board: Option<BoardId>,
}

impl From<SingleRequesterRecord> for Record {
    fn from(record: SingleRequesterRecord) -> Self {
        let request = Request {
            sequence: record.revision.sequence,
            answer: record.answer,
        };
        Self {
            board: record.board,
            requests: [(record.revision.writer, request)].into(),
        }
    }
}

// Ledgers written before boards had several requesters hold one revision per board.
fn decode(bytes: &[u8]) -> Option<Record> {
    p2panda_core::cbor::decode_cbor_strict(bytes)
        .ok()
        .or_else(|| {
            let single: Option<SingleRequesterRecord> =
                p2panda_core::cbor::decode_cbor_strict(bytes).ok();
            single.map(Record::from)
        })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CarriedBoard {
    pub handle: String,
    pub requested_by: DeviceId,
    pub removal_pending: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayDrop {
    Handle(String),
    From(DeviceId),
}

impl RelayDrop {
    fn selects(&self, key: &[u8], requester: DeviceId) -> bool {
        match self {
            Self::Handle(wanted) => *wanted == handle(key),
            Self::From(device) => requester == *device,
        }
    }
}

// A replayed addition must not outrank the drop. A revision that no relay clock accepts is
// pulled back so that its device can ask again.
fn dropped_sequence(requested: u64, now_ms: u64) -> u64 {
    if requested > now_ms.saturating_add(CLOCK_TOLERANCE_MS) {
        now_ms
    } else {
        requested.max(now_ms)
    }
}

const HANDLE_CONTEXT: &[u8] = b"pepino relay board handle v1";

// The operator names a board by this handle, which reveals neither the board ID nor its ledger key.
fn handle(key: &[u8]) -> String {
    let mut bytes = HANDLE_CONTEXT.to_vec();
    bytes.extend_from_slice(key);
    let mut handle = Hash::digest(bytes).to_hex();
    handle.truncate(12);
    handle
}

pub(super) struct Ledger {
    pub(super) records: BTreeMap<Vec<u8>, Record>,
    pub(super) rejected: u64,
    stored_rejected: u64,
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
        let rejected = rejected as u64;
        Ok(Self {
            records,
            rejected,
            stored_rejected: rejected,
            trusted,
            cap,
            pool,
        })
    }

    pub(super) fn is_trusted(&self, id: DeviceId) -> bool {
        self.trusted.contains(&id)
    }

    pub(super) fn reject(&mut self) -> RelayAnswer {
        self.rejected = (self.rejected + 1).min(i64::MAX as u64);
        RelayAnswer::NotTrusted
    }

    pub(super) async fn flush(&mut self) -> Result<(), Error> {
        if self.rejected != self.stored_rejected {
            sqlx::query("UPDATE pepino_relay_stats SET rejected = ?")
                .bind(self.rejected as i64)
                .execute(&self.pool)
                .await
                .map_err(Error::storage)?;
            self.stored_rejected = self.rejected;
        }
        Ok(())
    }

    pub(super) fn sequence(&self, board: BoardId, requester: DeviceId) -> u64 {
        self.records
            .get(&key(board))
            .and_then(|record| record.requests.get(&requester))
            .map_or(0, |request| request.sequence)
    }

    pub(super) fn counts(&self) -> BTreeMap<DeviceId, usize> {
        let mut counts: BTreeMap<_, _> = self.trusted.iter().map(|&id| (id, 0)).collect();
        for (requester, _) in self.records.values().flat_map(Record::requesters) {
            *counts.entry(requester).or_default() += 1;
        }
        counts
    }

    pub(super) async fn apply(
        &mut self,
        writer: DeviceId,
        instruction: RelayInstruction,
        now_ms: u64,
    ) -> Result<RelayAnswer, Error> {
        if !self.is_trusted(writer) {
            return Ok(self.reject());
        }
        if instruction.sequence > now_ms.saturating_add(CLOCK_TOLERANCE_MS) {
            return Ok(RelayAnswer::ClockAhead);
        }
        let key = key(instruction.board);
        let key = key.as_slice();
        let below_capacity = self
            .records
            .values()
            .filter(|record| record.board.is_some())
            .count()
            < self.cap;
        let (next, answer) =
            Record::next(self.records.get(key), writer, instruction, below_capacity);
        if let Some(next) = next {
            self.store(key, next).await?;
        }
        Ok(answer)
    }

    async fn store(&mut self, key: &[u8], record: Record) -> Result<(), Error> {
        sqlx::query("INSERT OR REPLACE INTO pepino_relay_ledger VALUES (?, ?)")
            .bind(key)
            .bind(encode(&record)?)
            .execute(&self.pool)
            .await
            .map_err(Error::storage)?;
        self.records.insert(key.to_vec(), record);
        Ok(())
    }

    pub(super) fn boards(&self) -> Vec<CarriedBoard> {
        self.records
            .iter()
            .filter(|(_, record)| record.board.is_some())
            .flat_map(|(key, record)| {
                record
                    .requests
                    .iter()
                    .filter(|(_, request)| request.answer != RelayAnswer::Removed)
                    .map(|(&requested_by, request)| CarriedBoard {
                        handle: handle(key),
                        requested_by,
                        removal_pending: request.answer == RelayAnswer::RemovalPendingRestart,
                    })
            })
            .collect()
    }

    /// Ends every selected request and returns how many boards that touched. A board left without
    /// requesters is purged at the next node start.
    pub(super) async fn drop_boards(
        &mut self,
        selection: &RelayDrop,
        now_ms: u64,
    ) -> Result<usize, Error> {
        let selected: Vec<_> = self
            .records
            .iter()
            .filter_map(|(key, record)| {
                let leaving: Vec<_> = record
                    .requesters()
                    .filter(|&(requester, _)| selection.selects(key, requester))
                    .map(|(requester, request)| {
                        (requester, dropped_sequence(request.sequence, now_ms))
                    })
                    .collect();
                (!leaving.is_empty()).then(|| {
                    let mut record = record.clone();
                    record.withdraw(&leaving);
                    (key.clone(), record)
                })
            })
            .collect();
        let boards = selected.len();
        for (key, record) in selected {
            self.store(&key, record).await?;
        }
        Ok(boards)
    }
}

fn key(board: BoardId) -> Vec<u8> {
    Hash::digest(board.as_bytes()).as_bytes().to_vec()
}

pub(super) async fn prepare(pool: &SqlitePool) -> Result<(), Error> {
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS pepino_relay_ledger (key BLOB PRIMARY KEY, record BLOB NOT NULL) STRICT;
         CREATE TABLE IF NOT EXISTS pepino_relay_stats (singleton INTEGER PRIMARY KEY CHECK (singleton = 1), rejected INTEGER NOT NULL) STRICT;
         INSERT OR IGNORE INTO pepino_relay_stats VALUES (1, 0);"
    ).execute(pool).await.map_err(Error::storage)?;
    let records = load_records(pool).await?;
    super::relay_purge::pending(pool, records).await?;
    super::relay_purge::checkpoint(pool).await.map(|_| ())
}

async fn load_records(pool: &SqlitePool) -> Result<BTreeMap<Vec<u8>, Record>, Error> {
    sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>("SELECT key, record FROM pepino_relay_ledger")
        .fetch_all(pool)
        .await
        .map_err(Error::storage)?
        .into_iter()
        .map(|(key, record)| {
            decode(&record)
                .map(|record| (key, record))
                .ok_or_else(|| Error::Storage("invalid relay ledger record".into()))
        })
        .collect()
}

pub(super) fn encode(record: &Record) -> Result<Vec<u8>, Error> {
    p2panda_core::cbor::encode_cbor(record)
        .map_err(|_| Error::Storage("could not encode relay ledger record".into()))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use proptest::prelude::*;

    pub(crate) const NOW: u64 = 1_795_300_000_000;

    pub(crate) async fn open(path: &std::path::Path) -> SqlitePool {
        let pool = crate::node::open_database(path, true).await.unwrap();
        p2panda_store::sqlite::run_pending_migrations(&pool)
            .await
            .unwrap();
        prepare(&pool).await.unwrap();
        pool
    }

    proptest! {
        #[test]
        fn replays_in_any_order_converge_to_each_requesters_latest_instruction(
            order in any::<[u8; 8]>(),
            latest_carries in any::<[bool; 2]>(),
        ) {
            let board = BoardId::from_bytes([42; 32]);
            let authors = [DeviceId::from_bytes([61; 32]), DeviceId::from_bytes([73; 32])];
            let mut deliveries: Vec<_> = order.into_iter().enumerate().collect();
            deliveries.sort_by_key(|(_, value)| *value);
            let mut record = Record {
                board: Some(board),
                requests: authors
                    .map(|author| (author, Request { sequence: 0, answer: RelayAnswer::Carrying }))
                    .into(),
            };
            for (index, _) in deliveries {
                let author = index % 2;
                let carry = if index >= 6 { latest_carries[author] } else { index % 4 < 2 };
                let instruction = RelayInstruction { board, sequence: 1 + (index / 2) as u64, carry };
                let (next, _) = Record::next(Some(&record), authors[author], instruction, true);
                record = next.unwrap_or(record);
            }
            for (author, carries) in authors.into_iter().zip(latest_carries) {
                let request = record.requests[&author];
                prop_assert_eq!(request.sequence, 4);
                prop_assert_eq!(request.answer == RelayAnswer::Carrying, carries);
                let old_add = RelayInstruction { board, sequence: 3, carry: true };
                prop_assert_eq!(
                    Record::next(Some(&record), author, old_add, true),
                    (None, RelayAnswer::Superseded)
                );
            }
            prop_assert_eq!(record.removal_pending(), latest_carries == [false, false]);
        }
    }

    #[tokio::test]
    async fn trust_and_capacity_gate_durable_admission() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&dir.path().join("relay.sqlite")).await;
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
            ledger.apply(stranger, add(57), NOW).await.unwrap(),
            RelayAnswer::NotTrusted
        );
        assert!(ledger.records.is_empty());
        let unknown_removal = RelayInstruction {
            carry: false,
            ..add(59)
        };
        assert_eq!(
            ledger.apply(trusted, unknown_removal, NOW).await.unwrap(),
            RelayAnswer::Removed
        );
        assert!(ledger.records.is_empty());
        assert_eq!(
            ledger.apply(trusted, add(57), NOW).await.unwrap(),
            RelayAnswer::Carrying
        );
        assert_eq!(
            ledger.apply(trusted, add(58), NOW).await.unwrap(),
            RelayAnswer::AtCapacity
        );
        assert_eq!(ledger.records.len(), 1);
        assert_eq!(
            ledger.apply(trusted, add(57), NOW).await.unwrap(),
            RelayAnswer::Carrying
        );
        let earlier = RelayInstruction {
            sequence: 0,
            ..add(57)
        };
        assert_eq!(
            ledger.apply(trusted, earlier, NOW).await.unwrap(),
            RelayAnswer::Superseded
        );
        assert_eq!(ledger.sequence(earlier.board, trusted), 1);
        assert_eq!(ledger.sequence(earlier.board, stranger), 0);
        assert_eq!(ledger.sequence(BoardId::from_bytes([58; 32]), trusted), 0);
        let restore = || Ledger::load(pool.clone(), [trusted].into(), 1);
        let restored = restore().await.unwrap();
        assert_eq!(restored.counts(), [(trusted, 1)].into());
        assert_eq!(restored.records.len(), 1);
        assert_eq!(restored.rejected, 0);
        for _ in 0..2 {
            assert_eq!(
                ledger.apply(stranger, add(58), NOW).await.unwrap(),
                RelayAnswer::NotTrusted
            );
        }
        assert_eq!(ledger.rejected, 3);
        assert_eq!(restore().await.unwrap().rejected, 0);
        ledger.flush().await.unwrap();
        assert_eq!(restore().await.unwrap().rejected, 3);
        let empty = Ledger::load(pool, [].into(), 64).await.unwrap();
        assert!(!empty.is_trusted(trusted));
    }

    #[tokio::test]
    async fn revisions_far_ahead_of_the_relay_clock_leave_no_record() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&dir.path().join("relay.sqlite")).await;
        let laptop = DeviceId::from_bytes([17; 32]);
        let mut ledger = Ledger::load(pool.clone(), [laptop].into(), 2)
            .await
            .unwrap();
        let greenhouse = BoardId::from_bytes([57; 32]);
        let instruction = |board, sequence, carry| RelayInstruction {
            board,
            sequence,
            carry,
        };
        let latest_allowed = NOW + CLOCK_TOLERANCE_MS;
        assert_eq!(
            ledger
                .apply(laptop, instruction(greenhouse, latest_allowed, true), NOW)
                .await
                .unwrap(),
            RelayAnswer::Carrying
        );
        let seed_house = BoardId::from_bytes([58; 32]);
        for (board, sequence, carry) in [
            (seed_house, latest_allowed + 1, true),
            (seed_house, u64::MAX, true),
            (greenhouse, u64::MAX, false),
        ] {
            assert_eq!(
                ledger
                    .apply(laptop, instruction(board, sequence, carry), NOW)
                    .await
                    .unwrap(),
                RelayAnswer::ClockAhead
            );
        }
        let restored = Ledger::load(pool, [laptop].into(), 2).await.unwrap();
        assert_eq!(restored.records.len(), 1);
        assert_eq!(restored.counts(), [(laptop, 1)].into());
        assert_eq!(
            ledger
                .apply(
                    laptop,
                    instruction(greenhouse, latest_allowed + 1, false),
                    NOW + 1
                )
                .await
                .unwrap(),
            RelayAnswer::RemovalPendingRestart
        );
    }

    #[tokio::test]
    async fn the_operator_lists_and_drops_boards_by_handle_or_requester() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&dir.path().join("relay.sqlite")).await;
        let laptop = DeviceId::from_bytes([17; 32]);
        let phone = DeviceId::from_bytes([27; 32]);
        let trusted: BTreeSet<_> = [laptop, phone].into();
        let mut ledger = Ledger::load(pool.clone(), trusted.clone(), 8)
            .await
            .unwrap();
        let add = |byte, sequence| RelayInstruction {
            board: BoardId::from_bytes([byte; 32]),
            sequence,
            carry: true,
        };
        for (device, byte) in [(laptop, 57), (laptop, 58), (phone, 59)] {
            assert_eq!(
                ledger.apply(device, add(byte, 1), NOW).await.unwrap(),
                RelayAnswer::Carrying
            );
        }
        let pinned = key(add(58, 1).board);
        let mut far_future = ledger.records[&pinned].clone();
        far_future.requests.get_mut(&laptop).unwrap().sequence = u64::MAX;
        ledger.store(&pinned, far_future).await.unwrap();

        let boards = ledger.boards();
        assert_eq!(boards.len(), 3);
        let handles: BTreeSet<_> = boards.iter().map(|board| board.handle.clone()).collect();
        assert_eq!(handles.len(), 3);
        for board in &boards {
            assert_eq!(board.handle.len(), 12);
            assert!(!board.removal_pending);
            for byte in [57, 58, 59] {
                let id = BoardId::from_bytes([byte; 32]);
                assert!(!id.to_string().contains(&board.handle));
                assert!(!Hash::digest(id.as_bytes()).to_hex().contains(&board.handle));
            }
        }
        assert_eq!(
            boards
                .iter()
                .filter(|board| board.requested_by == laptop)
                .count(),
            2
        );

        let unknown = RelayDrop::Handle("0123456789ab".into());
        assert_eq!(ledger.drop_boards(&unknown, NOW).await.unwrap(), 0);
        let stranger = RelayDrop::From(DeviceId::from_bytes([99; 32]));
        assert_eq!(ledger.drop_boards(&stranger, NOW).await.unwrap(), 0);
        let pinned_handle = RelayDrop::Handle(handle(&pinned));
        assert_eq!(ledger.drop_boards(&pinned_handle, NOW).await.unwrap(), 1);
        assert_eq!(
            ledger
                .boards()
                .iter()
                .filter(|board| board.removal_pending)
                .count(),
            1
        );
        let from_phone = RelayDrop::From(phone);
        assert_eq!(ledger.drop_boards(&from_phone, NOW).await.unwrap(), 1);
        drop(ledger);

        prepare(&pool).await.unwrap();
        let mut ledger = Ledger::load(pool, trusted, 8).await.unwrap();
        assert_eq!(ledger.counts(), [(laptop, 1), (phone, 0)].into());
        assert_eq!(ledger.boards().len(), 1);
        assert_eq!(ledger.sequence(add(58, 1).board, laptop), NOW);
        assert_eq!(
            ledger
                .apply(laptop, add(58, NOW + 1), NOW + 1)
                .await
                .unwrap(),
            RelayAnswer::Carrying
        );
    }

    #[tokio::test]
    async fn a_board_stays_until_its_last_requester_leaves() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&dir.path().join("relay.sqlite")).await;
        let alice = DeviceId::from_bytes([17; 32]);
        let bob = DeviceId::from_bytes([27; 32]);
        let carol = DeviceId::from_bytes([37; 32]);
        let trusted: BTreeSet<_> = [alice, bob, carol].into();
        let mut ledger = Ledger::load(pool.clone(), trusted.clone(), 1)
            .await
            .unwrap();
        let greenhouse = BoardId::from_bytes([57; 32]);
        let instruction = |sequence, carry| RelayInstruction {
            board: greenhouse,
            sequence,
            carry,
        };
        for (device, sequence) in [(bob, NOW), (alice, NOW + 1)] {
            assert_eq!(
                ledger
                    .apply(device, instruction(sequence, true), NOW + 1)
                    .await
                    .unwrap(),
                RelayAnswer::Carrying
            );
        }
        assert_eq!(ledger.counts(), [(alice, 1), (bob, 1), (carol, 0)].into());
        let listed = |ledger: &Ledger| -> Vec<_> {
            ledger
                .boards()
                .into_iter()
                .map(|board| (board.requested_by, board.removal_pending))
                .collect()
        };
        assert_eq!(listed(&ledger), [(alice, false), (bob, false)]);
        let handles: BTreeSet<_> = ledger.boards().into_iter().map(|b| b.handle).collect();
        assert_eq!(handles.len(), 1);

        assert_eq!(
            ledger
                .apply(carol, instruction(NOW + 2, false), NOW + 2)
                .await
                .unwrap(),
            RelayAnswer::Removed
        );
        assert_eq!(listed(&ledger), [(alice, false), (bob, false)]);

        let bob_leaves = instruction(NOW + 2, false);
        for _ in 0..2 {
            assert_eq!(
                ledger.apply(bob, bob_leaves, NOW + 2).await.unwrap(),
                RelayAnswer::Removed
            );
        }
        assert_eq!(listed(&ledger), [(alice, false)]);
        assert_eq!(ledger.counts(), [(alice, 1), (bob, 0), (carol, 0)].into());
        assert_eq!(
            ledger
                .apply(bob, instruction(NOW + 1, true), NOW + 2)
                .await
                .unwrap(),
            RelayAnswer::Superseded
        );
        assert_eq!(ledger.sequence(greenhouse, bob), NOW + 2);
        drop(ledger);

        prepare(&pool).await.unwrap();
        let mut ledger = Ledger::load(pool.clone(), trusted.clone(), 1)
            .await
            .unwrap();
        assert_eq!(listed(&ledger), [(alice, false)]);
        assert_eq!(
            ledger
                .apply(alice, instruction(NOW + 3, false), NOW + 3)
                .await
                .unwrap(),
            RelayAnswer::RemovalPendingRestart
        );
        assert_eq!(listed(&ledger), [(alice, true)]);
        assert_eq!(ledger.counts(), [(alice, 0), (bob, 0), (carol, 0)].into());
        drop(ledger);

        prepare(&pool).await.unwrap();
        let mut ledger = Ledger::load(pool, trusted, 1).await.unwrap();
        assert!(ledger.boards().is_empty());
        for (device, leaves) in [(alice, instruction(NOW + 3, false)), (bob, bob_leaves)] {
            assert_eq!(
                ledger.apply(device, leaves, NOW + 4).await.unwrap(),
                RelayAnswer::Removed
            );
        }
        let seed_house = RelayInstruction {
            board: BoardId::from_bytes([58; 32]),
            sequence: NOW + 4,
            carry: true,
        };
        assert_eq!(
            ledger.apply(carol, seed_house, NOW + 4).await.unwrap(),
            RelayAnswer::Carrying
        );
    }

    #[tokio::test]
    async fn dropping_one_requester_keeps_a_board_others_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&dir.path().join("relay.sqlite")).await;
        let alice = DeviceId::from_bytes([17; 32]);
        let bob = DeviceId::from_bytes([27; 32]);
        let trusted: BTreeSet<_> = [alice, bob].into();
        let mut ledger = Ledger::load(pool.clone(), trusted.clone(), 8)
            .await
            .unwrap();
        let add = |byte| RelayInstruction {
            board: BoardId::from_bytes([byte; 32]),
            sequence: NOW,
            carry: true,
        };
        let (shared, bobs, both_dropped) = (add(57), add(58), add(59));
        for (device, instruction) in [
            (alice, shared),
            (bob, shared),
            (bob, bobs),
            (alice, both_dropped),
            (bob, both_dropped),
        ] {
            assert_eq!(
                ledger.apply(device, instruction, NOW).await.unwrap(),
                RelayAnswer::Carrying
            );
        }
        let listed = |ledger: &Ledger| -> BTreeSet<_> {
            ledger
                .boards()
                .into_iter()
                .map(|board| (board.handle, board.requested_by, board.removal_pending))
                .collect()
        };
        let handle_of = |instruction: RelayInstruction| handle(&key(instruction.board));

        let from_bob = RelayDrop::From(bob);
        assert_eq!(ledger.drop_boards(&from_bob, NOW + 1).await.unwrap(), 3);
        assert_eq!(ledger.drop_boards(&from_bob, NOW + 1).await.unwrap(), 0);
        assert_eq!(
            listed(&ledger),
            [
                (handle_of(shared), alice, false),
                (handle_of(bobs), bob, true),
                (handle_of(both_dropped), alice, false),
            ]
            .into()
        );
        let by_handle = RelayDrop::Handle(handle_of(both_dropped));
        assert_eq!(ledger.drop_boards(&by_handle, NOW + 1).await.unwrap(), 1);
        assert_eq!(ledger.counts(), [(alice, 1), (bob, 0)].into());
        drop(ledger);

        prepare(&pool).await.unwrap();
        let mut ledger = Ledger::load(pool.clone(), trusted.clone(), 8)
            .await
            .unwrap();
        assert_eq!(listed(&ledger), [(handle_of(shared), alice, false)].into());

        let everyone = RelayDrop::Handle(handle_of(shared));
        let rejoined = RelayInstruction {
            sequence: NOW + 2,
            ..shared
        };
        assert_eq!(
            ledger.apply(bob, rejoined, NOW + 2).await.unwrap(),
            RelayAnswer::Carrying
        );
        assert_eq!(ledger.drop_boards(&everyone, NOW + 3).await.unwrap(), 1);
        assert_eq!(
            listed(&ledger),
            [
                (handle_of(shared), alice, true),
                (handle_of(shared), bob, true),
            ]
            .into()
        );
        drop(ledger);
        prepare(&pool).await.unwrap();
        let ledger = Ledger::load(pool, trusted, 8).await.unwrap();
        assert!(ledger.boards().is_empty());
        assert!(ledger.records.values().all(|record| record.board.is_none()));
    }

    #[tokio::test]
    async fn a_ledger_with_one_revision_per_board_keeps_its_boards_and_tombstones() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&dir.path().join("relay.sqlite")).await;
        let laptop = DeviceId::from_bytes([17; 32]);
        let phone = DeviceId::from_bytes([27; 32]);
        let greenhouse = BoardId::from_bytes([57; 32]);
        let seed_house = BoardId::from_bytes([58; 32]);
        for (board, record) in [
            (
                greenhouse,
                SingleRequesterRecord {
                    revision: RelayRevision {
                        sequence: NOW,
                        writer: laptop,
                    },
                    answer: RelayAnswer::Carrying,
                    board: Some(greenhouse),
                },
            ),
            (
                seed_house,
                SingleRequesterRecord {
                    revision: RelayRevision {
                        sequence: NOW + 5,
                        writer: phone,
                    },
                    answer: RelayAnswer::Removed,
                    board: None,
                },
            ),
        ] {
            sqlx::query("INSERT INTO pepino_relay_ledger VALUES (?, ?)")
                .bind(key(board))
                .bind(p2panda_core::cbor::encode_cbor(&record).unwrap())
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO pepino_relay_ledger VALUES (x'00', x'a0')")
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            Ledger::load(pool.clone(), [laptop, phone].into(), 8).await,
            Err(Error::Storage(_))
        ));
        sqlx::query("DELETE FROM pepino_relay_ledger WHERE key = x'00'")
            .execute(&pool)
            .await
            .unwrap();

        let mut ledger = Ledger::load(pool, [laptop, phone].into(), 8).await.unwrap();
        assert_eq!(ledger.counts(), [(laptop, 1), (phone, 0)].into());
        assert_eq!(ledger.sequence(greenhouse, laptop), NOW);
        let replayed = RelayInstruction {
            board: seed_house,
            sequence: NOW + 4,
            carry: true,
        };
        assert_eq!(
            ledger.apply(phone, replayed, NOW + 6).await.unwrap(),
            RelayAnswer::Superseded
        );
        let leave = RelayInstruction {
            board: greenhouse,
            sequence: NOW + 6,
            carry: false,
        };
        assert_eq!(
            ledger.apply(laptop, leave, NOW + 6).await.unwrap(),
            RelayAnswer::RemovalPendingRestart
        );
    }

    #[tokio::test]
    async fn a_replayed_addition_does_not_undo_an_operator_drop() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&dir.path().join("relay.sqlite")).await;
        let laptop = DeviceId::from_bytes([17; 32]);
        let mut ledger = Ledger::load(pool.clone(), [laptop].into(), 8)
            .await
            .unwrap();
        let seed_house = BoardId::from_bytes([58; 32]);
        let add = |sequence| RelayInstruction {
            board: seed_house,
            sequence,
            carry: true,
        };
        let stamped_ahead = add(NOW + 300_000);
        assert_eq!(
            ledger.apply(laptop, stamped_ahead, NOW).await.unwrap(),
            RelayAnswer::Carrying
        );
        let by_handle = RelayDrop::Handle(handle(&key(seed_house)));
        assert_eq!(
            ledger.drop_boards(&by_handle, NOW + 1_000).await.unwrap(),
            1
        );
        assert_eq!(
            ledger
                .apply(laptop, stamped_ahead, NOW + 2_000)
                .await
                .unwrap(),
            RelayAnswer::RemovalPendingRestart
        );
        drop(ledger);

        prepare(&pool).await.unwrap();
        let mut ledger = Ledger::load(pool, [laptop].into(), 8).await.unwrap();
        for (replayed, answer) in [
            (stamped_ahead, RelayAnswer::Removed),
            (add(NOW + 100), RelayAnswer::Superseded),
        ] {
            assert_eq!(
                ledger.apply(laptop, replayed, NOW + 2_000).await.unwrap(),
                answer
            );
        }
        assert!(ledger.boards().is_empty());
        assert_eq!(ledger.sequence(seed_house, laptop), NOW + 300_000);
        assert_eq!(
            ledger
                .apply(laptop, add(NOW + 300_001), NOW + 3_000)
                .await
                .unwrap(),
            RelayAnswer::Carrying
        );
    }
}
