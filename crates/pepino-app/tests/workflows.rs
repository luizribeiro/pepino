use pepino_app::{
    App, AppError, EdgeEffect, EdgeExclusionReason, EdgeKind, FieldValue, ReadRequest, Request,
    Response, TaskRecord,
};
use pepino_store::Store;
use serde_json::{Value, json};

const NOW: u64 = 1_795_144_800_000;

async fn handle(app: &mut App, request: Value) -> Result<Response, AppError> {
    app.handle(serde_json::from_value(request).unwrap(), NOW)
        .await
}

async fn write(
    app: &mut App,
    request: Value,
) -> (bool, Option<String>, Vec<TaskRecord>, Vec<EdgeEffect>) {
    let Response::Write {
        changed,
        created,
        tasks,
        edges,
    } = handle(app, request).await.unwrap()
    else {
        panic!("expected write response");
    };
    (changed, created, tasks, edges)
}

async fn add(app: &mut App, title: &str) -> String {
    let (changed, created, tasks, _) = write(app, json!({"op":"add_task","title":title})).await;
    assert!(changed);
    assert_eq!(created.as_deref(), Some(tasks[0].id.as_str()));
    assert_eq!(tasks[0].title, title);
    created.unwrap()
}

async fn get(app: &mut App, id: &str) -> TaskRecord {
    let Response::Task { task, .. } = app
        .handle(Request::Read(ReadRequest::GetTask { task: id.into() }), NOW)
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

    let (changed, created, tasks, edges) = write(&mut app, json!({
        "op":"add_task", "title":"Replace the greenhouse latch", "description":"Use stainless hardware.",
        "priority":2, "due":"2026-12-01", "tags":["greenhouse","garden/tools"], "assignees":["me",&assignee],
        "fields":{"estimate":{"type":"integer","value":3}}, "parent":&parent[..12],
        "blocked_by":[&blocker[..12]], "blocks":[&following,&following[..12]]
    })).await;
    assert!(changed);
    assert_eq!(tasks.len(), 4);
    let child = tasks
        .iter()
        .find(|task| task.title == "Replace the greenhouse latch")
        .unwrap();
    let id = created.unwrap();
    assert_eq!(child.id, id);
    assert_eq!(
        edges
            .iter()
            .map(|edge| (edge.from.as_str(), edge.kind, edge.to.as_str()))
            .collect::<Vec<_>>(),
        [
            (id.as_str(), EdgeKind::Parent, parent.as_str()),
            (id.as_str(), EdgeKind::Blocks, following.as_str()),
            (blocker.as_str(), EdgeKind::Blocks, id.as_str()),
        ],
    );
    assert!(
        edges
            .iter()
            .all(|edge| edge.effective && edge.reason.is_none())
    );
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
            field: "parent".into(),
            task: "deadbeef".into()
        }
    );
    assert!(store.tasks(board).await.unwrap().is_empty());
    assert_eq!(store.latest_hlc(board).await.unwrap(), None);
}

#[tokio::test]
async fn creation_rejects_the_same_dependency_in_both_directions() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    let first = add(&mut app, "Order a replacement latch").await;
    let second = add(&mut app, "Check the door seal").await;
    let store = Store::open(directory.path().join("pepino.sqlite"))
        .await
        .unwrap();
    let board = app.local_state().board_id;
    let before = store.latest_hlc(board).await.unwrap();

    let error = handle(
        &mut app,
        json!({
            "op":"add_task", "title":"Install the latch",
            "blocks":[&first], "blocked_by":[&second,first[..12].to_uppercase()]
        }),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, AppError::InvalidInput { field, .. } if field == "blocked_by[1]"));
    assert_eq!(store.latest_hlc(board).await.unwrap(), before);
    assert_eq!(store.tasks(board).await.unwrap().len(), 2);
}

#[tokio::test]
async fn edit_fields_completes_work_and_clears_optional_values() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    let id = add(&mut app, "Order a replacement latch").await;
    let (_, _, tasks, _) = write(
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
            "title":{"type":"text","value":"Order a stainless latch"}, "description":{"type":"text","value":"Use the local supplier."},
            "due":{"type":"date","value":"2026-12-01"}, "priority":{"type":"integer","value":2},
            "status":{"type":"status","value":"done"}, "estimate":{"type":"integer","value":1}
        }
    });
    let (changed, created, tasks, _) = write(&mut app, edit.clone()).await;
    assert!(changed && tasks.iter().any(|task| task.id == child));
    assert_eq!(created, None);
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
        (false, None, vec![done.clone()], Vec::new())
    );

    let invalid = json!({
        "op":"set_fields", "task":&id,
        "changes":{"description":{"type":"unset"},"title":{"type":"integer","value":3}}
    });
    assert!(
        matches!(handle(&mut app, invalid).await, Err(AppError::InvalidInput { field, .. }) if field == "changes.title")
    );
    assert_eq!(get(&mut app, &id).await, done);
    let invalid = json!({"op":"set_fields","task":&id,"changes":{"priority":{"type":"text","value":"urgent"}}});
    let error = handle(&mut app, invalid).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "invalid changes.priority: incorrect value type for built-in field"
    );

    let clear = json!({
        "op":"set_fields", "task":&id,
        "changes":{"description":{"type":"unset"},"due":{"type":"unset"},"estimate":{"type":"unset"},"priority":{"type":"unset"}}
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
        let (changed, created, new, _) = write(&mut app, request.clone()).await;
        assert!(changed);
        assert_eq!(created, None);
        assert_eq!(new, [get(&mut app, &id).await]);
        assert_eq!(
            write(&mut app, request).await,
            (false, None, new, Vec::new())
        );
    }
    let before = get(&mut app, &id).await;
    assert_eq!(before.tags, ["maintenance".into()].into());
    assert_eq!(before.assignees, [other].into());

    for (request, expected) in [
        (
            json!({"op":"add_tags","task":&id,"tags":["garden", "z INVALID"]}),
            "tags[1]",
        ),
        (
            json!({"op":"remove_tags","task":&id,"tags":["maintenance", "z INVALID"]}),
            "tags[1]",
        ),
        (
            json!({"op":"assign","task":&id,"assignees":["me","Ada"]}),
            "assignees[1]",
        ),
        (
            json!({"op":"unassign","task":&id,"assignees":["me","Ada"]}),
            "assignees[1]",
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
    assert_eq!(related.2.len(), 2);
    assert_eq!(get(&mut app, &second).await.relates, [first.clone()].into());
    assert!(!write(&mut app, link(&second, "relates", &first)).await.0);
    assert!(write(&mut app, unlink(&second, "relates", &first)).await.0);
    assert!(get(&mut app, &first).await.relates.is_empty());
    assert!(!write(&mut app, unlink(&first, "relates", &second)).await.0);

    assert!(write(&mut app, link(&first, "blocks", &second)).await.0);
    let (changed, _, _, edges) = write(&mut app, link(&second, "blocks", &first)).await;
    assert!(changed);
    let excluded = EdgeEffect {
        from: second.clone(),
        kind: EdgeKind::Blocks,
        to: first.clone(),
        effective: false,
        reason: Some(EdgeExclusionReason::Cycle),
    };
    assert_eq!(edges, std::slice::from_ref(&excluded));
    let response = handle(&mut app, json!({"op":"get_task","task":first}))
        .await
        .unwrap();
    assert!(matches!(response, Response::Task { edges, .. } if edges == [excluded.clone()]));
    assert!(get(&mut app, &first).await.blocked_by.is_empty());
    let (changed, _, _, edges) = write(&mut app, link(&second, "blocks", &first)).await;
    assert!(!changed && edges.is_empty());
    let (changed, _, _, edges) = write(&mut app, unlink(&second, "blocks", &first)).await;
    assert!(changed);
    assert_eq!(
        edges,
        [EdgeEffect {
            reason: None,
            ..excluded
        }]
    );
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
        let (changed, _, tasks, _) =
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
        edges: vec![],
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
    let (_, _, tasks, _) = write(
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

    let (changed, created, tasks, _) =
        write(&mut app, json!({"op":"delete","task":&blocker[..12]})).await;
    assert!(changed);
    assert_eq!(created, None);
    let remaining = get(&mut app, &child).await;
    assert_eq!(tasks, std::slice::from_ref(&remaining));
    assert!(remaining.blocked_by.is_empty() && remaining.ready);
    for (request, field) in [
        (json!({"op":"get_task","task":&blocker}), "task"),
        (
            json!({"op":"delete","task":blocker[..12].to_uppercase()}),
            "task",
        ),
        (
            json!({"op":"unlink","from":&blocker[..12],"kind":"blocks","to":&child}),
            "from",
        ),
    ] {
        assert_eq!(
            handle(&mut app, request).await.unwrap_err(),
            AppError::Conflict {
                field: field.into(),
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
            ..OpenOptions::default()
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
    let latest = pepino_app::Hlc::from(store.latest_hlc(local.board_id).await.unwrap().unwrap());
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
        Some(after.updated.into())
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
    assert_eq!(
        store.latest_hlc(board).await.unwrap(),
        Some(before.updated.into())
    );
    assert_eq!(store.tasks(board).await.unwrap().len(), 1);

    sqlx::query("DROP TRIGGER reject_edge")
        .execute(&mut connection)
        .await
        .unwrap();
    assert!(write(&mut app, request).await.0);
    assert_eq!(store.tasks(board).await.unwrap().len(), 2);
    assert_eq!(get(&mut app, &parent).await.children.len(), 1);
}

#[tokio::test]
async fn a_projection_failure_after_the_commit_reports_that_the_write_landed() {
    use sqlx::{ConnectOptions, sqlite::SqliteConnectOptions};

    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    let id = add(&mut app, "Prepare the greenhouse for winter").await;
    let database = directory.path().join("pepino.sqlite");
    let mut connection = SqliteConnectOptions::new()
        .filename(&database)
        .connect()
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER corrupt_title AFTER INSERT ON task_register WHEN NEW.kind = 'comment'
         BEGIN
           UPDATE task_register SET value = x'00'
           WHERE board = NEW.board AND task = NEW.task AND kind = 'field' AND member = x'7469746c65';
         END",
    )
    .execute(&mut connection)
    .await
    .unwrap();

    let error = handle(
        &mut app,
        json!({"op":"comment","task":&id,"text":"The new latch arrived."}),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&error, AppError::WriteCommitted { reason } if reason.contains("invalid task view value"))
    );
    assert!(!error.retryable());
    let store = Store::open(&database).await.unwrap();
    let history = store
        .events(app.local_state().board_id, id.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
}

async fn read_golden(app: &mut App, request: Value, expected: Value) {
    use pepino_app::{WireFormat, WireRequest, WireResponse};
    let request = json!({"v":1,"request":request});
    let expected = json!({"v":1,"result":expected});
    let wire: WireRequest = WireFormat::Json
        .decode(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    assert_eq!(serde_json::to_value(&wire).unwrap(), request);
    for codec in [WireFormat::Json, WireFormat::Cbor] {
        assert_eq!(
            codec
                .decode::<WireRequest>(&codec.encode(&wire).unwrap())
                .unwrap(),
            wire
        );
    }
    let response = WireResponse::new(app.handle(wire.request, NOW).await);
    assert_eq!(serde_json::to_value(&response).unwrap(), expected);
    for codec in [WireFormat::Json, WireFormat::Cbor] {
        assert_eq!(
            codec
                .decode::<WireResponse>(&codec.encode(&response).unwrap())
                .unwrap(),
            response
        );
    }
}

#[tokio::test]
async fn graph_reads_render_the_project_forest_and_transitive_dependencies() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    read_golden(
        &mut app,
        json!({"op":"tree","task":null}),
        json!({"ok":{"kind":"tree","nodes":[]}}),
    )
    .await;
    let root = add(&mut app, "Winterize the greenhouse").await;
    let order = add(&mut app, "Order the latch").await;
    let (_, install, _, _) = write(
        &mut app,
        json!({"op":"add_task","title":"Install the latch","parent":root,"blocked_by":[order]}),
    )
    .await;
    let install = install.unwrap();
    let (_, test, _, _) = write(&mut app, json!({"op":"add_task","title":"Test the door seal","parent":install,"blocked_by":[install]})).await;
    let test = test.unwrap();
    write(
        &mut app,
        json!({"op":"comment","task":test,"text":"Check the corners with a feeler gauge"}),
    )
    .await;
    let [root_record, order_record, install_record, test_record] = [
        get(&mut app, &root).await,
        get(&mut app, &order).await,
        get(&mut app, &install).await,
        get(&mut app, &test).await,
    ];
    let branch = json!([
        {"depth":0,"task":root_record},
        {"depth":1,"task":install_record},
        {"depth":2,"task":test_record}
    ]);
    read_golden(
        &mut app,
        json!({"op":"tree","task":root}),
        json!({"ok":{"kind":"tree","nodes":branch}}),
    )
    .await;
    let mut nodes = branch.as_array().unwrap().clone();
    nodes.push(json!({"depth":0,"task":order_record}));
    read_golden(
        &mut app,
        json!({"op":"tree","task":null}),
        json!({"ok":{"kind":"tree","nodes":nodes}}),
    )
    .await;
    read_golden(
        &mut app,
        json!({"op":"deps","task":test}),
        json!({"ok":{"kind":"deps","task":test_record,"blockers":[
        {"distance":1,"task":install_record},{"distance":2,"task":order_record}
    ],"dependents":[]}}),
    )
    .await;
    read_golden(
        &mut app,
        json!({"op":"deps","task":order}),
        json!({"ok":{"kind":"deps","task":order_record,"blockers":[],"dependents":[
            {"distance":1,"task":install_record},{"distance":2,"task":test_record}
        ]}}),
    )
    .await;
    for op in ["tree", "deps"] {
        read_golden(
            &mut app,
            json!({"op":op,"task":"0000"}),
            json!({"error":{"kind":"not_found","field":"task","task":"0000"}}),
        )
        .await;
    }
}

#[tokio::test]
async fn status_reports_local_context_and_counts_from_the_log() {
    use pepino_app::OpenOptions;
    use pepino_store::IdSource;
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open_with_options(
        directory.path(),
        NOW,
        OpenOptions {
            display_name: Some("Ada Gardener".into()),
            board_name: Some("Greenhouse".into()),
            ..OpenOptions::default()
        },
    )
    .await
    .unwrap();
    let local = app.local_state();
    let board = local.board_id;
    let mut expected = json!({
        "kind":"status",
        "identity":{"id":local.identity_id.to_string(),"display_name":"Ada Gardener"},
        "device":{"id":local.device_id.to_string(),"name":local.device_name},
        "board":{"id":board.to_string(),"name":"Greenhouse"},
        "data_dir":directory.path(),"protocol_version":1,
        "counts":{"open":0,"closed":0,"events":0,"skipped":{"unknown_version":0,"unknown_event":0,"unknown_value":0,"invalid":0}}
    });
    read_golden(&mut app, json!({"op":"status"}), json!({"ok":expected})).await;
    add(&mut app, "Clean the greenhouse gutters").await;
    write(
        &mut app,
        json!({"op":"add_task","title":"Inspect the roof","status":"done"}),
    )
    .await;
    let store = Store::open(directory.path().join("pepino.sqlite"))
        .await
        .unwrap();
    store
        .append(board, &[0x82, 2, 0xf6], IdSource::Content)
        .await
        .unwrap();
    expected["counts"] = json!({"open":1,"closed":1,"events":3,"skipped":{"unknown_version":1,"unknown_event":0,"unknown_value":0,"invalid":0}});
    read_golden(&mut app, json!({"op":"status"}), json!({"ok":expected})).await;
}
