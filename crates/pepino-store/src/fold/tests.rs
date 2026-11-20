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
