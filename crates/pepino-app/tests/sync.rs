use pepino_app::{App, Response, TaskRecord, TaskSummary};
use pepino_core::BoardId;
use pepino_store::{EventRecord, IdSource, Store};
use proptest::prelude::*;
use serde_json::{Value, json};

const NOW: u64 = 1_795_144_800_000;

async fn dispatch(app: &mut App, operation: Value) -> Response {
    app.handle(serde_json::from_value(operation).unwrap(), NOW)
        .await
        .unwrap()
}

async fn create(app: &mut App, operation: Value, title: &str) -> TaskSummary {
    let Response::Write {
        changed: true,
        tasks,
        ..
    } = dispatch(app, operation).await
    else {
        panic!("expected a task creation");
    };
    tasks.into_iter().find(|task| task.title == title).unwrap()
}

async fn get(app: &mut App, id: &str) -> TaskRecord {
    let Response::Task { task, .. } = dispatch(app, json!({"op":"get_task","task":id})).await
    else {
        panic!("expected a task record");
    };
    *task
}

async fn receive(store: &Store, board: BoardId, events: &[EventRecord]) {
    let raw: Vec<_> = events
        .iter()
        .map(|event| (event.bytes.as_slice(), IdSource::Trusted(event.id)))
        .collect();
    store.append_batch(board, &raw).await.unwrap();
}

async fn exchange(order: [u8; 8]) -> ([TaskRecord; 2], [TaskRecord; 2]) {
    let left_dir = tempfile::tempdir().unwrap();
    let right_dir = tempfile::tempdir().unwrap();
    let mut left = App::open(left_dir.path(), NOW + 3 * 86_400_000)
        .await
        .unwrap();
    let mut right = App::open(right_dir.path(), NOW).await.unwrap();
    let left_store = Store::open(left_dir.path().join("pepino.sqlite"))
        .await
        .unwrap();
    let right_store = Store::open(right_dir.path().join("pepino.sqlite"))
        .await
        .unwrap();
    let left_board = left.local_state().board_id;
    let right_board = right.local_state().board_id;
    assert_ne!(left.local_state().device_id, right.local_state().device_id);

    let title = "Order a replacement greenhouse latch";
    let created = create(
        &mut left,
        json!({
            "op":"add_task","title":title,"tags":["greenhouse"],"assignees":["me"]
        }),
        title,
    )
    .await;
    let id = &created.id;
    let task = id.parse().unwrap();
    let first = left_store.events(left_board, task).await.unwrap();
    receive(&right_store, right_board, &first).await;
    assert_eq!(get(&mut right, id).await.summary, created);

    for (app, status, text) in [
        (&mut left, "doing", "The supplier has stainless latches."),
        (&mut right, "done", "I picked up the replacement."),
    ] {
        let result = dispatch(
            app,
            json!({
                "op":"set_fields","task":id,"changes":{"status":{"type":"status","value":status}}
            }),
        )
        .await;
        let Response::Write {
            changed: true,
            tasks,
            ..
        } = result
        else {
            panic!("expected a status write");
        };
        assert!(tasks[0].updated > created.updated);
        dispatch(app, json!({"op":"comment","task":id,"text":text})).await;
    }
    let child_title = "Install the latch";
    let child = create(
        &mut left,
        json!({
            "op":"add_task","title":child_title,"blocked_by":[id]
        }),
        child_title,
    )
    .await;

    let mut events = left_store.events(left_board, task).await.unwrap();
    events.extend(right_store.events(right_board, task).await.unwrap());
    events.extend(
        left_store
            .events(left_board, child.id.parse().unwrap())
            .await
            .unwrap(),
    );
    let mut ranked: Vec<_> = events.into_iter().enumerate().collect();
    ranked.sort_by_key(|(index, event)| (order[index % order.len()], event.id));
    let mut events: Vec<_> = ranked.into_iter().map(|(_, event)| event).collect();
    receive(&left_store, left_board, &events).await;
    events.reverse();
    receive(&right_store, right_board, &events).await;

    (
        [get(&mut left, id).await, get(&mut left, &child.id).await],
        [get(&mut right, id).await, get(&mut right, &child.id).await],
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn two_apps_converge_after_exchanging_raw_events(order in any::<[u8; 8]>()) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (left, right) = runtime.block_on(exchange(order));
        prop_assert_eq!(left[0].comments.len(), 2);
        prop_assert_eq!(&left[1].blocked_by, &[left[0].id.clone()].into());
        prop_assert_eq!(left, right);
    }
}
