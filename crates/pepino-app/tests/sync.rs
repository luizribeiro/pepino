#[path = "support/template.rs"]
mod fixture;

use pepino_app::{App, Response, TaskRecord, TaskSummary};
use pepino_core::BoardId;
use pepino_store::{EventRecord, Store};
use proptest::prelude::*;
use serde_json::{Value, json};

const NOW: u64 = 1_795_144_800_000;

fn sync_directory() -> tempfile::TempDir {
    #[cfg(target_os = "linux")]
    if std::env::var_os("TMPDIR").is_none()
        && let Ok(directory) = tempfile::tempdir_in("/dev/shm")
    {
        return directory;
    }
    tempfile::tempdir().unwrap()
}

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
        .map(|event| (event.bytes.as_slice(), Some(event.author)))
        .collect();
    store.append_batch(board, &raw).await.unwrap();
}

async fn exchange(order: [u8; 8]) -> ([TaskRecord; 2], [TaskRecord; 2]) {
    let left_dir = sync_directory();
    let right_dir = sync_directory();
    let mut left = fixture::open(left_dir.path(), NOW + 3 * 86_400_000)
        .await
        .unwrap();
    let left_board = left.local_state().board_id();
    fixture::prepare_board(right_dir.path(), left_board).await;
    let mut right = App::open(right_dir.path(), NOW).await.unwrap();
    let left_store = Store::open(left_dir.path().join("pepino.sqlite"))
        .await
        .unwrap();
    let right_store = Store::open(right_dir.path().join("pepino.sqlite"))
        .await
        .unwrap();
    let right_board = right.local_state().board_id();
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

#[tokio::test]
async fn unlink_reports_activation_of_a_dormant_remote_blocker() {
    let directory = sync_directory();
    let mut app = fixture::open(directory.path(), NOW).await.unwrap();
    let hinges = create(
        &mut app,
        json!({"op":"add_task","title":"Order hinges"}),
        "Order hinges",
    )
    .await;
    let door = create(
        &mut app,
        json!({"op":"add_task","title":"Fit the door","blocked_by":[&hinges.id]}),
        "Fit the door",
    )
    .await;
    let board = app.local_state().board_id();
    let store = Store::open(directory.path().join("pepino.sqlite"))
        .await
        .unwrap();
    let mut clock = pepino_core::Clock::from_hlc(store.latest_hlc(board).await.unwrap().unwrap());
    let remote = pepino_core::Envelope::new(
        pepino_core::DeviceId::from_bytes([42; 32]),
        clock.tick(NOW).unwrap(),
        pepino_core::Event::EdgeAdded {
            from: door.id.parse().unwrap(),
            kind: pepino_core::EdgeKind::Blocks,
            to: hinges.id.parse().unwrap(),
        },
    )
    .unwrap();
    store
        .append(board, &remote.encode().unwrap(), None)
        .await
        .unwrap();

    let response = dispatch(
        &mut app,
        json!({"op":"unlink","from":hinges.id,"kind":"blocks","to":door.id}),
    )
    .await;
    let Response::Write { activated, .. } = response else {
        panic!("expected a write")
    };
    assert_eq!(
        activated,
        [pepino_app::EdgeEffect {
            from: door.id,
            kind: pepino_app::EdgeKind::Blocks,
            to: hinges.id,
            effective: true,
            reason: None,
        }]
    );
}

#[tokio::test]
async fn deleting_a_live_task_cleans_up_edges_to_already_deleted_batch_targets() {
    let directory = sync_directory();
    let mut app = fixture::open(directory.path(), NOW).await.unwrap();
    let bench = create(
        &mut app,
        json!({"op":"add_task","title":"Repair the greenhouse bench"}),
        "Repair the greenhouse bench",
    )
    .await;
    let wood = create(
        &mut app,
        json!({"op":"add_task","title":"Pick up cedar","parent":bench.id}),
        "Pick up cedar",
    )
    .await;
    let board = app.local_state().board_id();
    let store = Store::open(directory.path().join("pepino.sqlite"))
        .await
        .unwrap();
    let mut clock = pepino_core::Clock::from_hlc(store.latest_hlc(board).await.unwrap().unwrap());
    let remote = pepino_core::Envelope::new(
        pepino_core::DeviceId::from_bytes([42; 32]),
        clock.tick(NOW).unwrap(),
        pepino_core::Event::TaskDeleted {
            task: bench.id.parse().unwrap(),
        },
    )
    .unwrap();
    store
        .append(board, &remote.encode().unwrap(), None)
        .await
        .unwrap();

    let response = dispatch(
        &mut app,
        json!({"op":"apply_to_tasks","tasks":[bench.id, wood.id],"change":{"type":"delete"}}),
    )
    .await;
    let Response::BatchWrite {
        outcome,
        changed_targets,
    } = response
    else {
        panic!("expected batch write")
    };
    assert_eq!(changed_targets, std::slice::from_ref(&wood.id));
    assert!(
        matches!(*outcome, Response::Write { edges, .. } if edges.len() == 1 && !edges[0].effective)
    );
    assert!(
        store
            .edges(board, wood.id.parse().unwrap())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn deletion_reports_roots_created_when_older_parent_edges_activate() {
    use pepino_core::{Clock, DeviceId, EdgeKind, Envelope, Event};

    let directory = sync_directory();
    let mut app = fixture::open(directory.path(), NOW).await.unwrap();
    let mut tasks = Vec::new();
    for title in [
        "Clear the old bed",
        "Order cedar",
        "Assemble the frame",
        "Paint the frame",
    ] {
        tasks.push(
            create(&mut app, json!({"op":"add_task","title":title}), title)
                .await
                .id,
        );
    }
    let [removed, order, build, paint] = tasks.as_slice() else {
        panic!("expected four tasks")
    };
    let board = app.local_state().board_id();
    let store = Store::open(directory.path().join("pepino.sqlite"))
        .await
        .unwrap();
    let mut clock = Clock::from_hlc(store.latest_hlc(board).await.unwrap().unwrap());
    for (from, to) in [
        (order, build),
        (paint, order),
        (build, paint),
        (order, removed),
    ] {
        let event = Event::EdgeAdded {
            from: from.parse().unwrap(),
            kind: EdgeKind::Parent,
            to: to.parse().unwrap(),
        };
        let envelope = Envelope::new(
            DeviceId::from_bytes([61; 32]),
            clock.tick(NOW).unwrap(),
            event,
        )
        .unwrap();
        store
            .append(board, &envelope.encode().unwrap(), None)
            .await
            .unwrap();
    }
    for request in [
        json!({"op":"preview_delete","tasks":[removed]}),
        json!({"op":"delete","task":removed}),
    ] {
        let response = dispatch(&mut app, request).await;
        let (Response::DeletePreview { top_level, .. } | Response::Write { top_level, .. }) =
            response
        else {
            panic!("expected deletion effects");
        };
        assert_eq!(top_level.len(), 1);
        assert_eq!(&top_level[0].id, build);
    }
}
