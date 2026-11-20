use pepino_app::{App, AppError, FieldValue, Request, Response, TaskRecord};
use pepino_store::Store;
use serde_json::{Value, json};

const NOW: u64 = 1_795_144_800_000;

async fn handle(app: &mut App, request: Value) -> Result<Response, AppError> {
    app.handle(serde_json::from_value(request).unwrap(), NOW)
        .await
}

async fn write(app: &mut App, request: Value) -> (bool, Vec<String>, Vec<TaskRecord>) {
    let Response::Write {
        changed,
        task_ids,
        tasks,
    } = handle(app, request).await.unwrap()
    else {
        panic!("expected write response");
    };
    (changed, task_ids, tasks)
}

async fn add(app: &mut App, title: &str) -> String {
    let (changed, ids, tasks) = write(app, json!({"op":"add_task","title":title})).await;
    assert!(changed);
    assert_eq!(ids, [tasks[0].id.clone()]);
    assert_eq!(tasks[0].title, title);
    ids.into_iter().next().unwrap()
}

async fn get(app: &mut App, id: &str) -> TaskRecord {
    let Response::Task { task } = app
        .handle(Request::GetTask { task: id.into() }, NOW)
        .await
        .unwrap()
    else {
        panic!("expected task response");
    };
    *task
}

#[tokio::test]
async fn add_plans_a_project_and_returns_each_affected_record() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    let parent = add(&mut app, "Prepare the greenhouse for winter").await;
    let blocker = add(&mut app, "Order a replacement latch").await;
    let following = add(&mut app, "Check the door seal").await;
    let assignee = app.local_state().identity_id.to_string();

    let (changed, ids, tasks) = write(&mut app, json!({
        "op":"add_task", "title":"Replace the greenhouse latch", "description":"Use stainless hardware.",
        "priority":2, "due":"2026-12-01", "tags":["greenhouse","garden/tools"], "assignees":["me",&assignee],
        "fields":{"estimate":{"integer":3}}, "parent":&parent[..12],
        "blocked_by":[&blocker[..12]], "blocks":[&following,&following[..12]]
    })).await;
    assert!(changed);
    assert_eq!(ids.len(), 4);
    let child = tasks
        .iter()
        .find(|task| task.title == "Replace the greenhouse latch")
        .unwrap();
    let id = child.id.clone();
    assert_eq!(*child, get(&mut app, &id).await);
    assert_eq!(child.parent.as_deref(), Some(parent.as_str()));
    assert_eq!(child.assignees, [assignee].into());
    assert_eq!(child.due.unwrap().to_string(), "2026-12-01");
    assert_eq!(child.fields["estimate"], FieldValue::Integer(3));
    assert_eq!(
        child.tags,
        ["garden/tools".into(), "greenhouse".into()].into()
    );
    assert!(child.blocked && !child.ready);
    assert_eq!(get(&mut app, &parent).await.children, [id.clone()].into());
    assert_eq!(get(&mut app, &blocker).await.blocks, [id.clone()].into());
    assert_eq!(get(&mut app, &following).await.blocked_by, [id].into());
}

#[tokio::test]
async fn a_missing_parent_leaves_the_event_log_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    let store = Store::open(directory.path().join("pepino.sqlite"))
        .await
        .unwrap();
    let board = app.local_state().board_id;
    let error = handle(
        &mut app,
        json!({
            "op":"add_task","title":"Replace the greenhouse latch","parent":"deadbeef"
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        AppError::NotFound {
            task: "deadbeef".into()
        }
    );
    assert!(store.tasks(board).await.unwrap().is_empty());
    assert_eq!(store.latest_hlc(board).await.unwrap(), None);
}

#[tokio::test]
async fn edit_fields_completes_work_and_clears_optional_values() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    let id = add(&mut app, "Order a replacement latch").await;
    let (_, _, tasks) = write(
        &mut app,
        json!({
            "op":"add_task", "title":"Install the latch", "blocked_by":[&id]
        }),
    )
    .await;
    let child = tasks
        .iter()
        .find(|task| task.title == "Install the latch")
        .unwrap()
        .id
        .clone();
    assert!(get(&mut app, &child).await.blocked);

    let edit = json!({
        "op":"set_fields", "task":&id[..12], "changes":{
            "title":{"text":"Order a stainless latch"}, "description":{"text":"Use the local supplier."},
            "due":{"date":"2026-12-01"}, "priority":{"integer":2},
            "status":{"status":"done"}, "estimate":{"integer":1}
        }
    });
    let (changed, ids, _) = write(&mut app, edit.clone()).await;
    assert!(changed && ids.contains(&child));
    let done = get(&mut app, &id).await;
    assert_eq!(done.title, "Order a stainless latch");
    assert_eq!(done.description.as_deref(), Some("Use the local supplier."));
    assert_eq!(done.priority, Some(2));
    assert_eq!(done.due.unwrap().to_string(), "2026-12-01");
    assert_eq!(done.status, pepino_app::Status::Done);
    assert_eq!(done.fields["estimate"], FieldValue::Integer(1));
    assert!(get(&mut app, &child).await.ready);
    assert_eq!(
        write(&mut app, edit).await,
        (false, vec![id.clone()], vec![done.clone()])
    );

    let invalid = json!({
        "op":"set_fields", "task":&id,
        "changes":{"description":"unset","title":{"integer":3}}
    });
    assert!(
        matches!(handle(&mut app, invalid).await, Err(AppError::InvalidInput { field, .. }) if field == "title")
    );
    assert_eq!(get(&mut app, &id).await, done);

    let clear = json!({
        "op":"set_fields", "task":&id,
        "changes":{"description":"unset","due":"unset","estimate":"unset","priority":"unset"}
    });
    assert!(write(&mut app, clear.clone()).await.0);
    let task = get(&mut app, &id).await;
    assert_eq!(
        (task.description, task.due, task.priority),
        (None, None, None)
    );
    assert!(task.fields.is_empty());
    assert!(!write(&mut app, clear).await.0);
}

#[tokio::test]
async fn tags_and_assignees_deduplicate_and_validate_entire_batches() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    let id = add(&mut app, "Sharpen the pruning shears").await;
    let me = app.local_state().identity_id.to_string();
    let other = "ab".repeat(32);
    let add_tags =
        json!({"op":"add_tags","task":&id,"tags":["garden/tools","garden/tools","maintenance"]});
    let remove_tags = json!({"op":"remove_tags","task":&id,"tags":["garden/tools","absent"]});
    let assign = json!({"op":"assign","task":&id,"assignees":["me",&me,other.to_uppercase()]});
    let unassign = json!({"op":"unassign","task":&id,"assignees":["me"]});

    for request in [add_tags, remove_tags, assign, unassign] {
        let (changed, ids, new) = write(&mut app, request.clone()).await;
        assert!(changed);
        assert_eq!(ids, std::slice::from_ref(&id));
        assert_eq!(new, [get(&mut app, &id).await]);
        assert_eq!(write(&mut app, request).await, (false, ids, new));
    }
    let before = get(&mut app, &id).await;
    assert_eq!(before.tags, ["maintenance".into()].into());
    assert_eq!(before.assignees, [other].into());

    for (request, expected) in [
        (
            json!({"op":"add_tags","task":&id,"tags":["garden", "z INVALID"]}),
            "tags",
        ),
        (
            json!({"op":"remove_tags","task":&id,"tags":["maintenance", "z INVALID"]}),
            "tags",
        ),
        (
            json!({"op":"assign","task":&id,"assignees":["me","Ada"]}),
            "assignees",
        ),
        (
            json!({"op":"unassign","task":&id,"assignees":["me","Ada"]}),
            "assignees",
        ),
    ] {
        assert!(
            matches!(handle(&mut app, request).await, Err(AppError::InvalidInput { field, .. }) if field == expected)
        );
        assert_eq!(get(&mut app, &id).await, before);
    }
}

#[tokio::test]
async fn link_and_unlink_use_present_edges_including_excluded_ones() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    let first = add(&mut app, "Order a replacement latch").await;
    let second = add(&mut app, "Install the latch").await;
    let link =
        |from: &str, kind: &str, to: &str| json!({"op":"link","from":from,"kind":kind,"to":to});
    let unlink =
        |from: &str, kind: &str, to: &str| json!({"op":"unlink","from":from,"kind":kind,"to":to});

    let related = write(&mut app, link(&first[..12], "relates", &second)).await;
    assert!(related.0);
    assert_eq!(related.1.len(), 2);
    assert_eq!(get(&mut app, &second).await.relates, [first.clone()].into());
    assert!(!write(&mut app, link(&second, "relates", &first)).await.0);
    assert!(write(&mut app, unlink(&second, "relates", &first)).await.0);
    assert!(get(&mut app, &first).await.relates.is_empty());
    assert!(!write(&mut app, unlink(&first, "relates", &second)).await.0);

    assert!(write(&mut app, link(&first, "blocks", &second)).await.0);
    assert!(write(&mut app, link(&second, "blocks", &first)).await.0);
    assert!(get(&mut app, &first).await.blocked_by.is_empty());
    assert!(!write(&mut app, link(&second, "blocks", &first)).await.0);
    assert!(write(&mut app, unlink(&second, "blocks", &first)).await.0);
    assert!(!write(&mut app, unlink(&second, "blocks", &first)).await.0);
    assert!(get(&mut app, &second).await.blocked);

    for request in [
        link(&first, "parent", &first),
        unlink(&first, "parent", &first),
    ] {
        assert!(
            matches!(handle(&mut app, request).await, Err(AppError::InvalidInput { field, .. }) if field == "to")
        );
    }
}

#[tokio::test]
async fn comments_are_ordered_and_preserve_repeated_text() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    let id = add(&mut app, "Install the latch").await;
    let author = app.local_state().device_id.to_string();
    let note = "The latch is in the tool cabinet.";
    for text in [note, note, "The door now closes flush."] {
        let (changed, _, tasks) =
            write(&mut app, json!({"op":"comment","task":&id,"text":text})).await;
        assert!(changed);
        let task = get(&mut app, &id).await;
        assert_eq!(tasks, std::slice::from_ref(&task));
        let comment = task.comments.last().unwrap();
        assert_eq!(
            (comment.author.as_str(), comment.text.as_str()),
            (author.as_str(), text)
        );
    }
    let task = get(&mut app, &id).await;
    assert_ne!(task.comments[0].id, task.comments[1].id);
    assert!(
        task.comments
            .windows(2)
            .all(|pair| pair[0].hlc < pair[1].hlc)
    );
    let oversized = json!({"op":"comment","task":&id,"text":"x".repeat(65_537)});
    assert!(
        matches!(handle(&mut app, oversized).await, Err(AppError::InvalidInput { field, .. }) if field == "text")
    );
    assert_eq!(get(&mut app, &id).await, task);

    let response = Response::Task {
        task: Box::new(task),
    };
    let json = serde_json::to_vec(&response).unwrap();
    assert_eq!(serde_json::from_slice::<Response>(&json).unwrap(), response);
    let mut cbor = Vec::new();
    ciborium::into_writer(&response, &mut cbor).unwrap();
    assert_eq!(
        ciborium::from_reader::<Response, _>(cbor.as_slice()).unwrap(),
        response
    );
}

#[tokio::test]
async fn deletion_conflicts_for_full_or_short_ids_and_unblocks_dependents() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    let blocker = add(&mut app, "Order a replacement latch").await;
    let (_, _, tasks) = write(
        &mut app,
        json!({
            "op":"add_task", "title":"Install the latch", "blocked_by":[&blocker]
        }),
    )
    .await;
    let child = tasks
        .iter()
        .find(|task| task.title == "Install the latch")
        .unwrap()
        .id
        .clone();
    assert!(get(&mut app, &child).await.blocked);

    let (changed, ids, tasks) = write(&mut app, json!({"op":"delete","task":&blocker[..12]})).await;
    assert!(changed && ids.contains(&blocker) && ids.contains(&child));
    let remaining = get(&mut app, &child).await;
    assert_eq!(tasks, std::slice::from_ref(&remaining));
    assert!(remaining.blocked_by.is_empty() && remaining.ready);
    for request in [
        json!({"op":"get_task","task":&blocker}),
        json!({"op":"delete","task":blocker[..12].to_uppercase()}),
        json!({"op":"unlink","from":&blocker[..12],"kind":"blocks","to":&child}),
    ] {
        assert_eq!(
            handle(&mut app, request).await.unwrap_err(),
            AppError::Conflict {
                task: blocker.clone()
            }
        );
    }
    let store = Store::open(directory.path().join("pepino.sqlite"))
        .await
        .unwrap();
    let history = store
        .events(app.local_state().board_id, blocker.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(history.len(), 3);
}

#[tokio::test]
async fn reopening_preserves_local_state_tasks_and_clock_across_wall_regression() {
    use pepino_app::OpenOptions;

    let directory = tempfile::tempdir().unwrap();
    let later = NOW + 3 * 86_400_000;
    let mut app = App::open_with_options(
        directory.path(),
        later,
        OpenOptions {
            display_name: Some("Ada Gardener".into()),
            board_name: Some("Greenhouse winter prep".into()),
        },
    )
    .await
    .unwrap();
    let local = app.local_state().clone();
    let id = add(&mut app, "Order a replacement latch").await;
    let recorded = get(&mut app, &id).await;
    let store = Store::open(directory.path().join("pepino.sqlite"))
        .await
        .unwrap();
    let latest = store.latest_hlc(local.board_id).await.unwrap().unwrap();
    assert_eq!(latest, recorded.updated);
    assert_eq!(latest.wall_ms, later);
    drop(app);

    let mut app = App::open(directory.path(), NOW).await.unwrap();
    assert_eq!(app.local_state(), &local);
    assert_eq!(get(&mut app, &id).await, recorded);
    assert!(
        write(
            &mut app,
            json!({"op":"comment","task":&id,"text":"The latch arrived."})
        )
        .await
        .0
    );
    let after = get(&mut app, &id).await;
    assert!(after.updated > latest);
    assert_eq!(after.comments[0].hlc, after.updated);
    assert_eq!(
        store.latest_hlc(local.board_id).await.unwrap(),
        Some(after.updated)
    );
}

#[tokio::test]
async fn a_failed_edge_projection_does_not_leave_a_task_or_advance_the_log() {
    use sqlx::{ConnectOptions, sqlite::SqliteConnectOptions};

    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    let parent = add(&mut app, "Prepare the greenhouse for winter").await;
    let database = directory.path().join("pepino.sqlite");
    let mut connection = SqliteConnectOptions::new()
        .filename(&database)
        .connect()
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_edge BEFORE INSERT ON task_register WHEN NEW.kind = 'edge'
         BEGIN SELECT RAISE(ABORT, 'injected edge projection failure'); END",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    let before = get(&mut app, &parent).await;
    let request = json!({"op":"add_task","title":"Replace the greenhouse latch","parent":&parent});
    let error = handle(&mut app, request.clone()).await.unwrap_err();
    assert!(
        matches!(error, AppError::Storage { retryable: false, reason } if reason.contains("injected edge projection failure"))
    );
    assert_eq!(get(&mut app, &parent).await, before);
    let store = Store::open(&database).await.unwrap();
    let board = app.local_state().board_id;
    assert_eq!(store.latest_hlc(board).await.unwrap(), Some(before.updated));
    assert_eq!(store.tasks(board).await.unwrap().len(), 1);

    sqlx::query("DROP TRIGGER reject_edge")
        .execute(&mut connection)
        .await
        .unwrap();
    assert!(write(&mut app, request).await.0);
    assert_eq!(store.tasks(board).await.unwrap().len(), 2);
    assert_eq!(get(&mut app, &parent).await.children.len(), 1);
}
