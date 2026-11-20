use std::sync::OnceLock;

use pepino_core::{
    BoardId, Date, DeviceId, Envelope, Event, EventId, Hlc, IdentityId, InitialFields, Status,
    TaskId,
};
use pepino_store::{AppendOutcome, BoardView, IdSource, Store};

const BOARD: BoardId = BoardId::from_bytes([91; 32]);
pub const ANA: IdentityId = IdentityId::from_bytes([21; 32]);
pub const BOB: IdentityId = IdentityId::from_bytes([23; 32]);

pub fn date(day: u32) -> Date {
    Date::from_ymd_opt(2026, 11, day).unwrap()
}

pub fn id(index: u8) -> TaskId {
    let mut bytes = [index + 1; 32];
    if index < 2 {
        bytes[..2].copy_from_slice(&[0xab, 0xcd]);
    }
    TaskId::from_bytes(bytes)
}

pub fn board() -> &'static BoardView {
    static VIEW: OnceLock<BoardView> = OnceLock::new();
    VIEW.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(load())
    })
}

async fn load() -> BoardView {
    let store = Store::open_in_memory().await.unwrap();
    let midnight = date(20).to_epoch_days() as u64 * 86_400_000;
    let rows = [
        (
            "Repair north fence",
            Status::Todo,
            Some(5),
            Some(20),
            &["garden", "hardware"][..],
            &[ANA][..],
        ),
        (
            "Order hinges",
            Status::Doing,
            Some(5),
            Some(19),
            &["hardware"],
            &[ANA, BOB],
        ),
        (
            "Treat the Straße gate",
            Status::Done,
            Some(20),
            Some(18),
            &["garden"],
            &[BOB],
        ),
        ("Retire bent latch", Status::Cancelled, None, None, &[], &[]),
    ];
    for (index, (title, status, priority, due, tags, assignees)) in (0u8..).zip(rows) {
        let fields = InitialFields {
            status,
            priority,
            due: due.map(date),
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            assignees: assignees.iter().copied().collect(),
            ..InitialFields::default()
        };
        let wall_ms = match index {
            0 | 7 => midnight - 1,
            11 => midnight,
            13 => u64::MAX,
            _ => midnight - 86_401_000 + u64::from(index),
        };
        record(
            &store,
            index,
            wall_ms,
            EventId::from_bytes(*id(index).as_bytes()),
            Event::TaskCreated {
                title: title.into(),
                fields,
            },
        )
        .await;
    }
    store.board(BOARD).await.unwrap()
}

async fn record(store: &Store, index: u8, wall_ms: u64, event_id: EventId, event: Event) {
    let author = DeviceId::from_bytes([index % 3 + 1; 32]);
    let hlc = Hlc {
        wall_ms,
        counter: u32::from(index),
    };
    let bytes = Envelope::new(author, hlc, event).unwrap().encode().unwrap();
    assert!(matches!(
        store
            .append(BOARD, &bytes, IdSource::Trusted(event_id))
            .await
            .unwrap(),
        AppendOutcome::Applied { .. }
    ));
}
