use pepino_core::{DeviceId, EventId, Hlc, InitialFields};

use super::*;
use crate::{IdSource, Store};

#[tokio::test]
async fn only_creation_makes_a_task_visible_and_deletion_is_permanent() {
    let board = BoardId::from_bytes([19; 32]);
    let store = Store::open_in_memory().await.unwrap();
    let creation = Envelope::new(
        DeviceId::from_bytes([34; 32]),
        Hlc {
            wall_ms: 1_795_144_800_010,
            counter: 1,
        },
        Event::TaskCreated {
            title: "Repair the shed door".into(),
            fields: InitialFields::default(),
        },
    )
    .unwrap();
    let task = creation.created_task().unwrap();
    let records = [
        (
            Event::TagAdded {
                task,
                tag: "garden".into(),
            },
            20,
            51,
        ),
        (creation.event().clone(), 10, 34),
        (Event::TaskDeleted { task }, 5, 27),
        (
            Event::TagAdded {
                task,
                tag: "next-week".into(),
            },
            30,
            51,
        ),
    ];
    for (index, (event, millis, author)) in records.into_iter().enumerate() {
        let envelope = Envelope::new(
            DeviceId::from_bytes([author; 32]),
            Hlc {
                wall_ms: 1_795_144_800_000 + millis,
                counter: 1,
            },
            event,
        )
        .unwrap();
        let id = if index == 1 {
            task.0
        } else {
            EventId::from_bytes([index as u8; 32])
        };
        store
            .append(board, &envelope.encode().unwrap(), IdSource::Trusted(id))
            .await
            .unwrap();
        let visible: Option<([u8; 12], [u8; 12], [u8; 32])> = sqlx::query_as(
            "SELECT created, updated, author FROM visible_tasks WHERE board = ? AND task = ?",
        )
        .bind(board.as_bytes().as_slice())
        .bind(task.as_bytes().as_slice())
        .fetch_optional(&mut *store.connection.lock().await)
        .await
        .unwrap();
        let updated = Hlc {
            wall_ms: 1_795_144_800_020,
            counter: 1,
        };
        let expected = (index == 1).then_some((
            creation.hlc().to_key(),
            updated.to_key(),
            *creation.author().as_bytes(),
        ));
        assert_eq!(visible, expected);
    }
}

#[tokio::test]
async fn field_register_uses_clock_then_id_and_keeps_unset() {
    use pepino_core::{FieldName, FieldValue};

    let store = Store::open_in_memory().await.unwrap();
    let board = BoardId::from_bytes([19; 32]);
    let task = TaskId::from_bytes([70; 32]);
    let hlc = Hlc {
        wall_ms: 1_795_144_800_000,
        counter: 4,
    };
    let field = FieldName::new("description").unwrap();
    let original = FieldValue::Text("Measure the door before buying hinges".into());
    let newer = FieldValue::Text("Use the brass hinges from the shed".into());
    for (id, author, counter, value, expected) in [
        (11, 32, 3, original.clone(), Some(original.clone())),
        (22, 51, 4, newer.clone(), Some(newer.clone())),
        (17, 12, 4, original.clone(), Some(newer)),
        (23, 12, 4, FieldValue::Unset, None),
        (90, 32, 3, original, None),
    ] {
        let envelope = Envelope::new(
            DeviceId::from_bytes([author; 32]),
            Hlc { counter, ..hlc },
            Event::TaskFieldSet {
                task,
                field: field.clone(),
                value,
            },
        )
        .unwrap();
        store
            .append(
                board,
                &envelope.encode().unwrap(),
                IdSource::Trusted(EventId::from_bytes([id; 32])),
            )
            .await
            .unwrap();
        let stored: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT value FROM task_register WHERE board = ? AND task = ?
             AND kind = 'field' AND member = ?",
        )
        .bind(board.as_bytes().as_slice())
        .bind(task.as_bytes().as_slice())
        .bind(field.as_str().as_bytes())
        .fetch_one(&mut *store.connection.lock().await)
        .await
        .unwrap();
        let decoded = stored.map(|bytes| ciborium::from_reader(bytes.as_slice()).unwrap());
        assert_eq!(decoded, expected);
    }
}

#[tokio::test]
async fn failed_register_write_rolls_back_the_log() {
    let store = Store::open_in_memory().await.unwrap();
    let board = BoardId::from_bytes([19; 32]);
    let envelope = Envelope::new(
        DeviceId::from_bytes([32; 32]),
        Hlc {
            wall_ms: 1_795_144_800_000,
            counter: 0,
        },
        Event::TaskCreated {
            title: "Seal the balcony rail".into(),
            fields: InitialFields::default(),
        },
    )
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_field BEFORE INSERT ON task_register
                WHEN NEW.member = x'737461747573'
                BEGIN SELECT RAISE(ABORT, 'test write failure'); END",
    )
    .execute(&mut *store.connection.lock().await)
    .await
    .unwrap();
    let bytes = envelope.encode().unwrap();
    assert!(matches!(
        store.append(board, &bytes, IdSource::Content).await,
        Err(StoreError::Database {
            kind: crate::DatabaseErrorKind::Constraint,
            ..
        })
    ));
    assert_eq!(store.latest_hlc(board).await.unwrap(), None);
    let tasks: i64 = sqlx::query_scalar("SELECT count(*) FROM task_state")
        .fetch_one(&mut *store.connection.lock().await)
        .await
        .unwrap();
    assert_eq!(tasks, 0);
}

#[tokio::test]
async fn tag_and_assignee_registers_converge_in_reverse_arrival_order() {
    use pepino_core::IdentityId;

    let board = BoardId::from_bytes([19; 32]);
    let task = TaskId::from_bytes([70; 32]);
    let who = IdentityId::from_bytes([29; 32]);
    let events = [
        Event::TagAdded {
            task,
            tag: "garden".into(),
        },
        Event::TagRemoved {
            task,
            tag: "garden".into(),
        },
        Event::TagAdded {
            task,
            tag: "garden".into(),
        },
        Event::AssigneeAdded { task, who },
        Event::AssigneeRemoved { task, who },
    ];
    let bytes: Vec<_> = events
        .into_iter()
        .zip([32, 51, 59, 32, 51])
        .enumerate()
        .map(|(index, (event, author))| {
            Envelope::new(
                DeviceId::from_bytes([author; 32]),
                Hlc {
                    wall_ms: 1_795_144_800_000 + index as u64,
                    counter: 0,
                },
                event,
            )
            .unwrap()
            .encode()
            .unwrap()
        })
        .collect();
    for order in [
        bytes.iter().collect::<Vec<_>>(),
        bytes.iter().rev().collect(),
    ] {
        let store = Store::open_in_memory().await.unwrap();
        for bytes in order {
            store.append(board, bytes, IdSource::Content).await.unwrap();
        }
        let rows: Vec<(String, Vec<u8>, Option<Vec<u8>>)> = sqlx::query_as(
            "SELECT kind, member, value FROM task_register WHERE board = ? AND task = ?
             ORDER BY kind, member",
        )
        .bind(board.as_bytes().as_slice())
        .bind(task.as_bytes().as_slice())
        .fetch_all(&mut *store.connection.lock().await)
        .await
        .unwrap();
        assert_eq!(
            rows,
            [
                ("assignee".into(), who.as_bytes().to_vec(), None),
                ("tag".into(), b"garden".to_vec(), Some(vec![])),
            ]
        );
    }
}

#[tokio::test]
async fn same_clock_comments_have_distinct_registers() {
    let board = BoardId::from_bytes([19; 32]);
    let task = TaskId::from_bytes([70; 32]);
    let store = Store::open_in_memory().await.unwrap();
    let clock = Hlc {
        wall_ms: 1_795_144_800_000,
        counter: 0,
    };
    for (id, author, text) in [
        (50, 32, "Bring a drill"),
        (40, 51, "Hinges are on the shelf"),
    ] {
        let envelope = Envelope::new(
            DeviceId::from_bytes([author; 32]),
            clock,
            Event::CommentAdded {
                task,
                text: text.into(),
            },
        )
        .unwrap();
        store
            .append(
                board,
                &envelope.encode().unwrap(),
                IdSource::Trusted(EventId::from_bytes([id; 32])),
            )
            .await
            .unwrap();
    }
    let rows: Vec<([u8; 32], Vec<u8>)> = sqlx::query_as(
        "SELECT id, value FROM task_register WHERE kind = 'comment' ORDER BY hlc, id",
    )
    .fetch_all(&mut *store.connection.lock().await)
    .await
    .unwrap();
    assert_eq!(
        rows,
        [
            ([40; 32], b"Hinges are on the shelf".to_vec()),
            ([50; 32], b"Bring a drill".to_vec())
        ]
    );
}
