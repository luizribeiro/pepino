use std::sync::OnceLock;

use pepino_core::{
    BoardId, Date, DeviceId, EdgeKind, Envelope, Event, FieldName, FieldValue, Hlc, IdentityId,
    InitialFields, Status, TaskId,
};
use pepino_store::{AppendOutcome, BoardView, IdSource, Store};

const BOARD: BoardId = BoardId::from_bytes([91; 32]);
pub const ANA: IdentityId = IdentityId::from_bytes([21; 32]);
pub const BOB: IdentityId = IdentityId::from_bytes([23; 32]);

pub fn date(day: u32) -> Date {
    Date::from_ymd_opt(2026, 11, day).unwrap()
}

pub fn id(index: u8) -> TaskId {
    fixture().1[usize::from(index)]
}

pub fn board() -> &'static BoardView {
    &fixture().0
}

fn fixture() -> &'static (BoardView, Vec<TaskId>) {
    static VIEW: OnceLock<(BoardView, Vec<TaskId>)> = OnceLock::new();
    VIEW.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(load())
    })
}

async fn load() -> (BoardView, Vec<TaskId>) {
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
        (
            "Weatherproof the shed",
            Status::Todo,
            Some(-2),
            None,
            &["hardware"],
            &[],
        ),
        ("Clean gutters", Status::Done, None, None, &[], &[]),
        (
            "Paint mailbox",
            Status::Todo,
            None,
            Some(20),
            &["paint"],
            &[],
        ),
        (
            "Stake tomatoes",
            Status::Doing,
            Some(5),
            Some(20),
            &["garden"],
            &[],
        ),
        (
            "Buy screws",
            Status::Todo,
            Some(5),
            None,
            &["hardware"],
            &[],
        ),
        ("Check porch light", Status::Todo, None, None, &[], &[]),
        ("Wash windows", Status::Done, Some(3), Some(21), &[], &[]),
        (
            "Cover raised beds",
            Status::Todo,
            Some(5),
            Some(20),
            &["garden"],
            &[],
        ),
        ("ΟΔΥΣΣΕΎΣ plaque", Status::Todo, None, None, &[], &[]),
        ("Drain hose", Status::Todo, None, None, &[], &[]),
        (
            "Deleted greenhouse",
            Status::Todo,
            None,
            None,
            &["garden"],
            &[],
        ),
    ];
    let mut ids = Vec::new();
    for (index, (title, status, priority, due, tags, assignees)) in (0u8..).zip(rows) {
        let mut fields = InitialFields {
            status,
            priority,
            due: due.map(date),
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            assignees: assignees.iter().copied().collect(),
            ..InitialFields::default()
        };
        if index == 0 {
            fields.description = Some("Use CÈDRE posts".into());
            fields.custom.extend([
                (FieldName::new("estimate").unwrap(), FieldValue::Integer(12)),
                (
                    FieldName::new("inspection").unwrap(),
                    FieldValue::Date(date(21)),
                ),
                (
                    FieldName::new("supplier").unwrap(),
                    FieldValue::Text("Acme".into()),
                ),
            ]);
        }
        if index == 1 {
            fields.custom.insert(
                FieldName::new("estimate").unwrap(),
                FieldValue::Text("12".into()),
            );
        }
        if index == 4 {
            fields.description = Some("Seal the roof first".into());
            fields
                .custom
                .insert(FieldName::new("estimate").unwrap(), FieldValue::Integer(8));
        }
        if index == 3 {
            fields.description = Some(String::new());
        }
        let wall_ms = match index {
            0 | 7 => midnight - 1,
            11 => midnight,
            13 => u64::MAX,
            _ => midnight - 86_401_000 + u64::from(index),
        };
        let creation = record(
            &store,
            index,
            wall_ms,
            Event::TaskCreated {
                title: title.into(),
                fields,
            },
        )
        .await;
        ids.push(creation.created_task().unwrap());
    }
    let id = |index: usize| ids[index];
    for (counter, field) in [(30, "description"), (31, "estimate")] {
        record(
            &store,
            counter,
            midnight - 86_400_990,
            Event::TaskFieldSet {
                task: id(4),
                field: FieldName::new(field).unwrap(),
                value: FieldValue::Unset,
            },
        )
        .await;
    }
    let edge = |from, kind, to| Event::EdgeAdded {
        from: id(from),
        kind,
        to: id(to),
    };
    for (index, event) in (40u8..).zip([
        edge(1, EdgeKind::Parent, 0),
        edge(1, EdgeKind::Parent, 7),
        edge(6, EdgeKind::Parent, 0),
        edge(9, EdgeKind::Parent, 14),
        edge(1, EdgeKind::Blocks, 0),
        edge(0, EdgeKind::Blocks, 6),
        edge(6, EdgeKind::Blocks, 1),
        edge(2, EdgeKind::Blocks, 8),
        edge(14, EdgeKind::Blocks, 9),
        Event::CommentAdded {
            task: id(0),
            text: "Bring galvanized bolts".into(),
        },
        Event::CommentAdded {
            task: id(7),
            text: "Use the Straße marker".into(),
        },
        Event::TaskDeleted { task: id(14) },
    ]) {
        record(&store, index, midnight, event).await;
    }
    (store.board_with_comments(BOARD).await.unwrap(), ids)
}

async fn record(store: &Store, index: u8, wall_ms: u64, event: Event) -> Envelope {
    let mut author = [index % 3 + 1; 32];
    // The first two creations share the abcd prefix used by the query tests.
    if let Some(nonce) = [114615_u32, 55642].get(usize::from(index)) {
        author[..4].copy_from_slice(&nonce.to_le_bytes());
    }
    let author = DeviceId::from_bytes(author);
    let hlc = Hlc {
        wall_ms,
        counter: u32::from(index),
    };
    let envelope = Envelope::new(author, hlc, event).unwrap();
    let bytes = envelope.encode().unwrap();
    assert!(matches!(
        store
            .append(BOARD, &bytes, IdSource::Content)
            .await
            .unwrap(),
        AppendOutcome::Applied { .. }
    ));
    envelope
}
