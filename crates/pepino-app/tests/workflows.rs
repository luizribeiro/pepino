use pepino_app::{
    App, AppError, EdgeEffect, EdgeExclusionReason, EdgeKind, FieldValue, ReadRequest, Request,
    Response, TaskRecord, TaskSummary,
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
) -> (bool, Option<String>, Vec<TaskSummary>, Vec<EdgeEffect>) {
    let Response::Write {
        changed,
        created,
        tasks,
        edges,
        names,
        ..
    } = handle(app, request).await.unwrap()
    else {
        panic!("expected write response");
    };
    if tasks
        .iter()
        .any(|task| task.author == app.local_state().device_id.to_string())
    {
        assert_eq!(
            names[&app.local_state().device_id.to_string()],
            app.local_state().device_name
        );
    }
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
    assert_eq!(*child, get(&mut app, &id).await.summary);
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
            reference: pepino_app::MissingReference::Task("deadbeef".into())
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
    let Response::Write {
        changed,
        created,
        tasks,
        targets,
        ..
    } = handle(&mut app, edit.clone()).await.unwrap()
    else {
        panic!("expected write response");
    };
    assert_eq!(targets, std::slice::from_ref(&id));
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
        (false, None, vec![done.summary.clone()], Vec::new())
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
        (task.description.as_deref(), task.due, task.priority),
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
        assert_eq!(new, [get(&mut app, &id).await.summary]);
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
    write(
        &mut app,
        json!({"op":"assign","task":first,"assignees":["me"]}),
    )
    .await;
    let response = handle(&mut app, json!({"op":"get_task","task":first}))
        .await
        .unwrap();
    let response = serde_json::to_value(response).unwrap();
    assert_eq!(response["edges"], json!([excluded]));
    assert_eq!(
        response["relation_titles"],
        json!({second.clone(): "Install the latch"})
    );
    assert_eq!(
        response["assignee_names"],
        json!({
            app.local_state().identity_id.to_string(): app.local_state().display_name
        })
    );
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
    for (index, text) in [note, note, "The door now closes flush."]
        .into_iter()
        .enumerate()
    {
        let (changed, _, tasks, _) =
            write(&mut app, json!({"op":"comment","task":&id,"text":text})).await;
        assert!(changed);
        let task = get(&mut app, &id).await;
        assert_eq!(tasks, std::slice::from_ref(&task.summary));
        assert_eq!(tasks[0].comment_count, (index + 1) as u64);
        assert!(
            serde_json::to_value(&tasks[0])
                .unwrap()
                .get("comments")
                .is_none()
        );
        let comment = task.comments.last().unwrap();
        assert_eq!(
            (comment.author.as_str(), comment.text.as_str()),
            (author.as_str(), text)
        );
    }
    let task = get(&mut app, &id).await;
    assert_eq!(task.comment_count, task.comments.len() as u64);
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

    let Response::Task { names, .. } = handle(&mut app, json!({"op":"get_task","task":id}))
        .await
        .unwrap()
    else {
        panic!("expected task detail");
    };
    assert_eq!(
        names,
        [(author, app.local_state().device_name.clone())].into()
    );
    let response = Response::Task {
        edges: vec![],
        task: Box::new(task),
        relation_titles: Default::default(),
        assignee_names: Default::default(),
        names: Default::default(),
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
    assert_eq!(tasks, std::slice::from_ref(&remaining.summary));
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
                reference: pepino_app::ConflictReference::Task {
                    task: blocker.clone()
                },
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
        get(&mut app, &root).await.summary,
        get(&mut app, &order).await.summary,
        get(&mut app, &install).await.summary,
        get(&mut app, &test).await.summary,
    ];
    assert_eq!(test_record.comment_count, 1);
    let names = json!({app.local_state().device_id.to_string(): app.local_state().device_name});
    let branch = json!([
        {"depth":0,"task":root_record},
        {"depth":1,"task":install_record},
        {"depth":2,"task":test_record}
    ]);
    read_golden(
        &mut app,
        json!({"op":"tree","task":root}),
        json!({"ok":{"kind":"tree","names":names,"nodes":branch}}),
    )
    .await;
    let mut nodes = branch.as_array().unwrap().clone();
    nodes.push(json!({"depth":0,"task":order_record}));
    read_golden(
        &mut app,
        json!({"op":"tree","task":null}),
        json!({"ok":{"kind":"tree","names":names,"nodes":nodes}}),
    )
    .await;
    read_golden(
        &mut app,
        json!({"op":"deps","task":test}),
        json!({"ok":{"kind":"deps","names":names,"task":test_record,"blockers":[
        {"distance":1,"task":install_record},{"distance":2,"task":order_record}
    ],"dependents":[]}}),
    )
    .await;
    read_golden(
        &mut app,
        json!({"op":"deps","task":order}),
        json!({"ok":{"kind":"deps","names":names,"task":order_record,"blockers":[],"dependents":[
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
        "counts":{"open":0,"closed":0,"events":0,"applied":0,"skipped":{"unknown_version":0,"unknown_event":0,"unknown_value":0,"invalid":0}}
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
    expected["counts"] = json!({"open":1,"closed":1,"events":3,"applied":2,"skipped":{"unknown_version":1,"unknown_event":0,"unknown_value":0,"invalid":0}});
    read_golden(&mut app, json!({"op":"status"}), json!({"ok":expected})).await;
}

#[tokio::test]
async fn history_reports_board_and_task_changes_including_skipped_envelopes() {
    use ciborium::Value as Cbor;
    use pepino_store::IdSource;
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    let board = app.local_state().board_id;
    let author = app.local_state().device_id.to_string();
    let task = add(&mut app, "Replace the greenhouse latch").await;
    write(
        &mut app,
        json!({"op":"comment","task":task,"text":"Bring\n stainless screws"}),
    )
    .await;
    let task_record = get(&mut app, &task).await;
    let comment = &task_record.comments[0];
    let store = Store::open(directory.path().join("pepino.sqlite"))
        .await
        .unwrap();
    let future = store
        .append(board, &[0x82, 2, 0xf6], IdSource::Content)
        .await
        .unwrap();
    let pepino_store::AppendOutcome::Skipped { id: future_id, .. } = future else {
        panic!("expected a skip")
    };
    let mut bytes = store
        .events(board, task.parse().unwrap())
        .await
        .unwrap()
        .pop()
        .unwrap()
        .bytes;
    let mut frame: Cbor = ciborium::from_reader(bytes.as_slice()).unwrap();
    let (_, event) = frame.as_array_mut().unwrap()[1]
        .as_map_mut()
        .unwrap()
        .iter_mut()
        .find(|(key, _)| key.as_text() == Some("event"))
        .unwrap();
    event.as_map_mut().unwrap()[0].0 = Cbor::from("reminder_added");
    bytes.clear();
    ciborium::into_writer(&frame, &mut bytes).unwrap();
    let skip_id = pepino_core::EventId::from_bytes([255; 32]);
    assert!(matches!(
        store
            .append(board, &bytes, IdSource::Trusted(skip_id))
            .await
            .unwrap(),
        pepino_store::AppendOutcome::Skipped { .. }
    ));
    let names = json!({app.local_state().device_id.to_string(): app.local_state().device_name});
    let unordered = json!([
        {"id":future_id.to_string(),"author":null,"hlc":null,"kind":"unknown","classification":"unknown_version","change":{"type":"skipped","summary":"Skipped event","version":2,"tasks":[]}},
    ]);
    let events = json!([
        {"id":task,"author":author,"hlc":task_record.created,"kind":"task_created","classification":"applied","change":{"type":"creation","action":"created","summary":"Created Replace the greenhouse latch","task":task,"title":"Replace the greenhouse latch","fields":{"description":null,"status":"todo","due":null,"priority":null,"tags":[],"assignees":[],"fields":{}}}},
        {"id":comment.id,"author":author,"hlc":comment.hlc,"kind":"comment_added","classification":"applied","change":{"type":"comment","action":"commented","summary":"Commented “Bring stainless screws”","task":task,"text":"Bring\n stainless screws"}},
        {"id":skip_id.to_string(),"author":author,"hlc":comment.hlc,"kind":"reminder_added","classification":"unknown_event","change":{"type":"skipped","summary":"Skipped event","tasks":[task]}}
    ]);
    read_golden(
        &mut app,
        json!({"op":"log","task":null,"limit":null}),
        json!({"ok":{"kind":"log","names":names,"titles":{(task.as_str()):"Replace the greenhouse latch"},"unordered":unordered,"events":events}}),
    )
    .await;
    read_golden(
        &mut app,
        json!({"op":"log","task":null,"limit":0}),
        json!({"ok":{"kind":"log","titles":{},"unordered":unordered,"events":[]}}),
    )
    .await;
    read_golden(
        &mut app,
        json!({"op":"log","task":task,"limit":2}),
        json!({"ok":{"kind":"log","names":names,"titles":{(task.as_str()):"Replace the greenhouse latch"},"unordered":[],"events":&events.as_array().unwrap()[1..]}}),
    )
    .await;
    read_golden(
        &mut app,
        json!({"op":"log","task":"0000","limit":null}),
        json!({"error":{"kind":"not_found","field":"task","task":"0000"}}),
    )
    .await;
}

#[tokio::test]
async fn saved_views_migrate_v1_state_and_survive_concurrent_opens() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("local.json");
    let mut legacy = json!({
        "version":1,"device_id":"51".repeat(32),"identity_id":"62".repeat(32),
        "display_name":"Ada Gardener","device_name":"Garden laptop","board_id":"73".repeat(32),
        "board_name":"Greenhouse"
    });
    std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    legacy["version"] = json!(2);
    legacy["saved_views"] = json!({});
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap(),
        legacy
    );
    read_golden(
        &mut app,
        json!({"op":"list_views"}),
        json!({"ok":{"kind":"views","views":[]}}),
    )
    .await;
    let mut peer = App::open(directory.path(), NOW).await.unwrap();
    assert!(
        serde_json::from_value::<Request>(json!({
            "op":"save_view","name":"latch","filter":"due:tomorrow","today":"2026-11-20"
        }))
        .is_err()
    );
    let latch = json!({"name":"latch","filter":"due:tomorrow tag:hardware"});
    read_golden(
        &mut app,
        json!({"op":"save_view","name":"latch","filter":["due:tomorrow","tag:hardware"]}),
        json!({"ok":{"kind":"view_saved","view":latch,"replaced":false}}),
    )
    .await;
    let weekend = json!({"name":"weekend","filter":"@latch is:open"});
    read_golden(
        &mut peer,
        json!({"op":"save_view","name":"weekend","filter":"@latch is:open"}),
        json!({"ok":{"kind":"view_saved","view":weekend,"replaced":false}}),
    )
    .await;
    let latch = json!({"name":"latch","filter":"tag:hinges"});
    read_golden(
        &mut app,
        json!({"op":"save_view","name":"latch","filter":"tag:hinges"}),
        json!({"ok":{"kind":"view_saved","view":latch,"replaced":true}}),
    )
    .await;
    drop(app);
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    read_golden(
        &mut app,
        json!({"op":"list_views"}),
        json!({"ok":{"kind":"views","views":[latch,weekend]}}),
    )
    .await;
    read_golden(
        &mut app,
        json!({"op":"delete_view","name":"latch"}),
        json!({"error":{"kind":"conflict","field":"name","view":"latch","referenced_by":["weekend"]}}),
    )
    .await;
    read_golden(
        &mut app,
        json!({"op":"delete_view","name":"latch","force":true}),
        json!({"ok":{"kind":"view_deleted","view":latch}}),
    )
    .await;
    read_golden(
        &mut peer,
        json!({"op":"delete_view","name":"latch"}),
        json!({"error":{"kind":"not_found","field":"name","view":"latch"}}),
    )
    .await;
    read_golden(
        &mut peer,
        json!({"op":"list_views"}),
        json!({"ok":{"kind":"views","views":[weekend]}}),
    )
    .await;
    for request in [
        json!({"op":"save_view","name":"Weekend Plan","filter":""}),
        json!({"op":"delete_view","name":"weekend.plan"}),
    ] {
        read_golden(&mut app, request, json!({"error":{"kind":"invalid_input","field":"name","reason":"expected [a-z][a-z0-9_-]* within 64 bytes"}})).await;
    }
    read_golden(&mut app, json!({"op":"save_view","name":"latch","filter":"tags:hardware"}), json!({"error":{"kind":"invalid_input","field":"filter","reason":"unknown filter key \"tags\"","hint":"Use \"tag\"","term":"tags:hardware","position":{"kind":"byte","byte":0}}})).await;
}

async fn list_golden(
    app: &mut App,
    filter: Value,
    sort: Option<&str>,
    limit: Option<usize>,
    expected: Value,
) {
    read_golden(
        app,
        json!({"op":"list_tasks","filter":filter,"sort":sort,"limit":limit,"today":"2026-11-20"}),
        expected,
    )
    .await;
}

#[tokio::test]
async fn saved_relative_dates_follow_each_listing_date() {
    let directory = tempfile::tempdir().unwrap();
    let early = "2026-11-12T12:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    let early = u64::try_from(early.timestamp_millis()).unwrap();
    let mut app = App::open(directory.path(), early).await.unwrap();
    for (title, due, now) in [
        ("Order weatherstripping", "2026-11-18", early),
        ("Check the old seal", "2026-11-25", early),
        ("Fit the replacement seal", "2026-11-25", NOW),
        ("Seal the vent frame", "2026-11-27", NOW),
    ] {
        let request = json!({"op":"add_task","title":title,"due":due});
        app.handle(serde_json::from_value(request).unwrap(), now)
            .await
            .unwrap();
    }

    let view = json!({"name":"maintenance","filter":"due:<fri due:tomorrow updated:>-1w"});
    read_golden(
        &mut app,
        json!({"op":"save_view","name":"maintenance","filter":["due:<friday","due:tomorrow","updated:>-01w"]}),
        json!({"ok":{"kind":"view_saved","view":view,"replaced":false}}),
    ).await;
    drop(app);
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    for (today, friday, tomorrow, cutoff, expected) in [
        (
            "2026-11-17",
            "2026-11-20",
            "2026-11-18",
            "2026-11-10",
            &["Order weatherstripping"][..],
        ),
        (
            "2026-11-24",
            "2026-11-27",
            "2026-11-25",
            "2026-11-17",
            &["Fit the replacement seal"][..],
        ),
        (
            "2026-11-26",
            "2026-11-27",
            "2026-11-27",
            "2026-11-19",
            &[][..],
        ),
    ] {
        let listing = handle(
            &mut app,
            json!({"op":"list_tasks","filter":"@maintenance","today":today}),
        )
        .await
        .unwrap();
        let Response::Tasks {
            tasks,
            filter,
            resolved_dates,
            total,
            ..
        } = listing
        else {
            panic!("expected task listing");
        };
        assert_eq!(filter, "@maintenance");
        assert_eq!(total, expected.len());
        assert_eq!(
            tasks
                .iter()
                .map(|task| task.title.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            serde_json::to_value(resolved_dates).unwrap(),
            json!({"fri":friday,"tomorrow":tomorrow,"-1w":cutoff}),
        );
        read_golden(
            &mut app,
            json!({"op":"list_views"}),
            json!({"ok":{"kind":"views","views":[view]}}),
        )
        .await;
    }
}

#[tokio::test]
async fn listing_reports_relative_overflow_on_the_offending_term() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    list_golden(
        &mut app,
        json!(["  updated:>+9223372036854775807w"]),
        None,
        None,
        json!({"error":{
            "kind":"invalid_input","field":"filter",
            "reason":"date \"+9223372036854775807w\" is outside the supported calendar range",
            "term":"updated:>+9223372036854775807w",
            "position":{"kind":"argument","index":0,"byte":2}
        }}),
    )
    .await;
}

#[tokio::test]
async fn task_lists_sort_limit_and_search_project_records_with_both_filter_forms() {
    use pepino_app::OpenOptions;
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
    list_golden(
        &mut app,
        json!(""),
        None,
        None,
        json!({"ok":{"kind":"tasks","tasks":[],"total":0,"filter":"","resolved_dates":{},"sort":"default"}}),
    )
    .await;
    let (_, latch, _, _) = write(&mut app,json!({"op":"add_task","title":"Replace the latch","tags":["hardware"],"assignees":["me"],"due":"2026-11-21","priority":9})).await;
    let (_, hinges, _, _) = write(&mut app,json!({"op":"add_task","title":"Inspect the hinges","tags":["hardware"],"assignees":["me"],"due":"2026-11-27","priority":1})).await;
    let latch = latch.unwrap();
    let hinges = get(&mut app, &hinges.unwrap()).await;
    add(&mut app, "Clean the gutters").await;
    write(
        &mut app,
        json!({"op":"comment","task":latch,"text":"Look for stainless screws"}),
    )
    .await;
    let latch = get(&mut app, &latch).await;
    let summary = latch.summary;
    assert_eq!(summary.comment_count, 1);
    let names = json!({
        app.local_state().identity_id.to_string(): "Ada Gardener",
        app.local_state().device_id.to_string(): app.local_state().device_name
    });

    list_golden(&mut app,json!("tag:hardware assignee:me"),None, None,
        json!({"ok":{"kind":"tasks","names":names,"tasks":[summary,hinges.summary],"total":2,"filter":"tag:hardware assignee:me","resolved_dates":{},"sort":"default"}})).await;
    list_golden(&mut app,json!(["tag:hardware","assignee:Ada Gardener"]),Some("title"),Some(1),
        json!({"ok":{"kind":"tasks","names":names,"tasks":[hinges.summary],"total":2,"filter":"tag:hardware assignee:\"Ada Gardener\"","resolved_dates":{},"sort":"title"}})).await;
    list_golden(&mut app,json!("due:tomorrow"),None,Some(0),
        json!({"ok":{"kind":"tasks","tasks":[],"total":1,"filter":"due:tomorrow","resolved_dates":{"tomorrow":"2026-11-21"},"sort":"default"}})).await;
    for filter in [
        json!(" tag:hardware  \"STAINLESS screws\" "),
        json!(["tag:hardware", "STAINLESS screws"]),
    ] {
        list_golden(&mut app,filter,None,None,
            json!({"ok":{"kind":"tasks","names":names,"tasks":[summary],"total":1,"filter":"tag:hardware \"STAINLESS screws\"","resolved_dates":{},"sort":"default"}})).await;
    }

    for (name, filter) in [
        (
            "latch",
            "board:Greenhouse,Workshop \"stainless screws\" due:tomorrow",
        ),
        ("weekend", "tag:hardware @latch"),
    ] {
        handle(
            &mut app,
            json!({"op":"save_view","name":name,"filter":filter}),
        )
        .await
        .unwrap();
    }
    drop(app);
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    list_golden(&mut app, json!("@weekend"), None, None,
        json!({"ok":{"kind":"tasks","names":names,"tasks":[summary],"total":1,"filter":"@weekend","resolved_dates":{"tomorrow":"2026-11-21"},"sort":"default"}})).await;
    list_golden(&mut app, json!("-board:Greenhouse @weekend"), None, None,
        json!({"ok":{"kind":"tasks","tasks":[],"total":0,"filter":"-board:Greenhouse @weekend","resolved_dates":{},"sort":"default"}})).await;
    list_golden(
        &mut app,
        json!("board:Workshop"),
        None,
        None,
        json!({"error":{"kind":"not_found","field":"filter","board":"Workshop"}}),
    )
    .await;
    handle(
        &mut app,
        json!({"op":"save_view","name":"remote","filter":"board:Workshop"}),
    )
    .await
    .unwrap();
    list_golden(
        &mut app,
        json!("@remote"),
        None,
        None,
        json!({"error":{"kind":"not_found","field":"filter","board":"Workshop"}}),
    )
    .await;
    list_golden(&mut app, json!("-@weekend"), None, None,
        json!({"error":{"kind":"invalid_input","field":"filter","reason":"negated saved view @weekend selects boards","term":"-@weekend","position":{"kind":"byte","byte":0}}})).await;
}

#[tokio::test]
async fn listing_errors_report_source_positions_and_saved_view_chains() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    list_golden(&mut app,json!(""),Some("-deadline"),None,
        json!({"error":{"kind":"invalid_input","field":"sort","reason":"unknown sort key \"deadline\""}})).await;
    list_golden(&mut app,json!(["is:open","  tags:hardware"]),None,None,
        json!({"error":{"kind":"invalid_input","field":"filter","reason":"unknown filter key \"tags\"","hint":"Use \"tag\"","term":"tags:hardware","position":{"kind":"argument","index":1,"byte":2}}})).await;
    list_golden(&mut app,json!(" \"é \\\"hinge\\\"\"  parent:bad"),None,None,
        json!({"error":{"kind":"invalid_input","field":"filter","reason":"invalid task ID prefix \"bad\" (expected 4 to 64 hexadecimal digits)","term":"parent:bad","position":{"kind":"byte","byte":" \"é \\\"hinge\\\"\"  ".len()}}})).await;
    let person_hint = format!("Known person is {}", app.local_state().display_name);
    list_golden(&mut app,json!(["is:open","  assignee:Ben Gardener"]),None,None,
        json!({"error":{"kind":"invalid_input","field":"filter","reason":"unknown person \"Ben Gardener\"","hint":person_hint,"term":"assignee:\"Ben Gardener\"","position":{"kind":"argument","index":1,"byte":2}}})).await;
    replace_saved_views(
        directory.path(),
        json!({"first":"@missing","last":"is:open @first"}),
    );
    list_golden(&mut app,json!("@first"),None,None,
        json!({"error":{"kind":"invalid_input","field":"filter","reason":"unknown saved view \"missing\"","hint":"Use list_views to see available saved views","term":"@missing","position":{"kind":"byte","byte":0},"view_chain":["first"]}})).await;
    replace_saved_views(
        directory.path(),
        json!({"first":"@last","last":"is:open @first"}),
    );
    list_golden(&mut app,json!("@first"),None,None,
        json!({"error":{"kind":"invalid_input","field":"filter","reason":"saved view cycle: first -> last -> first","term":"@first","position":{"kind":"byte","byte":8},"view_chain":["first","last","first"]}})).await;
}

#[tokio::test]
async fn history_describes_invalid_events_and_preserves_unknown_value_targets() {
    use ciborium::Value as Cbor;
    use pepino_core::{DeviceId, Envelope, Event, EventId, FieldName, FieldValue, Hlc, Status};
    use pepino_store::IdSource;

    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    let board = app.local_state().board_id;
    let app_task = add(&mut app, "Refit the greenhouse door").await;
    let task = app_task.parse().unwrap();
    let store = Store::open(directory.path().join("pepino.sqlite"))
        .await
        .unwrap();
    let peer = DeviceId::from_bytes([61; 32]);
    let hlc = Hlc {
        wall_ms: NOW,
        counter: 3,
    };
    let envelopes = [
        (
            Event::TagAdded {
                task,
                tag: "hardware".into(),
            },
            "tag",
            Cbor::from("Hardware"),
        ),
        (
            Event::TaskFieldSet {
                task,
                field: FieldName::new("status").unwrap(),
                value: FieldValue::Status(Status::Doing),
            },
            "value",
            Cbor::Map(vec![(Cbor::from("status"), Cbor::from("waiting"))]),
        ),
    ];
    let [invalid, unknown] = [EventId::from_bytes([28; 32]), EventId::from_bytes([29; 32])];
    for ((event, member, value), id) in envelopes.into_iter().zip([invalid, unknown]) {
        let encoded = Envelope::new(peer, hlc, event).unwrap().encode().unwrap();
        let mut frame: Cbor = ciborium::from_reader(encoded.as_slice()).unwrap();
        let content = frame.as_array_mut().unwrap()[1].as_map_mut().unwrap();
        let (_, event) = content
            .iter_mut()
            .find(|(key, _)| key.as_text() == Some("event"))
            .unwrap();
        let fields = event.as_map_mut().unwrap()[0].1.as_map_mut().unwrap();
        let (_, field) = fields
            .iter_mut()
            .find(|(key, _)| key.as_text() == Some(member))
            .unwrap();
        *field = value;
        let mut raw = Vec::new();
        ciborium::into_writer(&frame, &mut raw).unwrap();
        assert!(matches!(
            store
                .append(board, &raw, IdSource::Trusted(id))
                .await
                .unwrap(),
            pepino_store::AppendOutcome::Skipped { .. }
        ));
    }
    let wire_clock = pepino_app::Hlc::from(hlc);
    read_golden(&mut app, json!({"op":"log","task":app_task,"limit":2}), json!({"ok":{"kind":"log","titles":{(app_task.as_str()):"Refit the greenhouse door"},"unordered":[],"events":[
        {"id":invalid.to_string(),"author":peer.to_string(),"hlc":wire_clock,"classification":"invalid","kind":"tag_added","reason":"invalid tag \"Hardware\"","change":{"type":"tag","action":"added","summary":"Added tag Hardware","task":app_task,"tag":"Hardware"}},
        {"id":unknown.to_string(),"author":peer.to_string(),"hlc":wire_clock,"classification":"unknown_value","kind":"task_field_set","change":{"type":"skipped","summary":"Skipped event","tasks":[app_task]}}
    ]}})).await;
}

#[tokio::test]
async fn log_actions_distinguish_sets_unsets_and_presence_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::open(dir.path(), NOW).await.unwrap();
    let task = add(&mut app, "Replace the greenhouse latch").await;
    for request in [
        json!({"op":"set_fields","task":task,"changes":{"priority":{"type":"integer","value":3}}}),
        json!({"op":"set_fields","task":task,"changes":{"priority":{"type":"unset"}}}),
        json!({"op":"add_tags","task":task,"tags":["hardware"]}),
        json!({"op":"remove_tags","task":task,"tags":["hardware"]}),
        json!({"op":"delete","task":task}),
    ] {
        handle(&mut app, request).await.unwrap();
    }
    let Response::Log { events, titles, .. } = handle(&mut app, json!({"op":"log","task":task}))
        .await
        .unwrap()
    else {
        panic!("expected log");
    };
    assert_eq!(
        titles[&task].as_deref(),
        Some("Replace the greenhouse latch")
    );
    use pepino_app::EventAction::*;
    assert_eq!(
        events
            .iter()
            .filter_map(|event| event.change.action)
            .collect::<Vec<_>>(),
        [Created, Set, Unset, Added, Removed, Deleted]
    );
}

#[tokio::test]
async fn an_edge_change_identifies_its_source_task() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::open(dir.path(), NOW).await.unwrap();
    let from = add(&mut app, "Order latches").await;
    let to = add(&mut app, "Repair greenhouse door").await;
    handle(
        &mut app,
        json!({"op":"link","from":from,"kind":"blocks","to":to}),
    )
    .await
    .unwrap();
    let Response::Log { events, titles, .. } = handle(&mut app, json!({"op":"log","limit":1}))
        .await
        .unwrap()
    else {
        panic!("expected log");
    };
    assert_eq!(titles[&from].as_deref(), Some("Order latches"));
    assert_eq!(titles[&to].as_deref(), Some("Repair greenhouse door"));
    let change = serde_json::to_value(&events[0].change).unwrap();
    assert_eq!(change["task"], from);
    assert_eq!(change["from"], from);
    assert_eq!(change["to"], to);
    assert_eq!(change["action"], "added");
}

#[tokio::test]
async fn lists_only_parse_reachable_saved_filters() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::open(dir.path(), NOW).await.unwrap();
    replace_saved_views(
        dir.path(),
        json!({"weekend":"tag:hardware","broken":"tags:hardware","nested":"@broken"}),
    );
    list_golden(
        &mut app,
        json!("@weekend"),
        None,
        None,
        json!({"ok":{"kind":"tasks","tasks":[],"total":0,"filter":"@weekend","resolved_dates":{},"sort":"default"}}),
    )
    .await;
    list_golden(&mut app, json!("@nested"), None, None, json!({"error":{"kind":"invalid_input","field":"filter","reason":"unknown filter key \"tags\"","term":"tags:hardware","position":{"kind":"byte","byte":0},"view_chain":["nested","broken"],"hint":"Use \"tag\""}})).await;
}

fn replace_saved_views(directory: &std::path::Path, views: Value) {
    let path = directory.join("local.json");
    let mut local: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    local["saved_views"] = views;
    std::fs::write(path, serde_json::to_vec(&local).unwrap()).unwrap();
}

#[tokio::test]
async fn saving_rejects_missing_and_cyclic_references_without_persisting() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::open(dir.path(), NOW).await.unwrap();
    let save = |name, filter| json!({"op":"save_view","name":name,"filter":filter});
    handle(&mut app, save("first", "tag:hardware"))
        .await
        .unwrap();
    handle(&mut app, save("last", "is:open @first"))
        .await
        .unwrap();
    read_golden(&mut app, json!({"op":"save_view","name":"first","filter":["tag:hardware","  @missing"]}), json!({"error":{"kind":"invalid_input","field":"filter","reason":"unknown saved view \"missing\"","term":"@missing","position":{"kind":"argument","index":1,"byte":2},"view_chain":["first"],"hint":"Use list_views to see available saved views"}})).await;
    read_golden(&mut app, save("first", "@last"), json!({"error":{"kind":"invalid_input","field":"filter","reason":"saved view cycle: first -> last -> first","term":"@first","position":{"kind":"byte","byte":8},"view_chain":["first","last","first"]}})).await;
    read_golden(&mut app, json!({"op":"list_views"}), json!({"ok":{"kind":"views","views":[{"name":"first","filter":"tag:hardware"},{"name":"last","filter":"is:open @first"}]}})).await;
}

#[tokio::test]
async fn an_invalid_creation_lists_its_implicit_task_id_without_a_title() {
    use ciborium::Value as Cbor;
    use pepino_core::{DeviceId, Envelope, Event, Hlc, InitialFields};
    use pepino_store::IdSource;

    let dir = tempfile::tempdir().unwrap();
    let mut app = App::open(dir.path(), NOW).await.unwrap();
    let store = Store::open(dir.path().join("pepino.sqlite")).await.unwrap();
    let creation = Envelope::new(
        DeviceId::from_bytes([31; 32]),
        Hlc {
            wall_ms: NOW,
            counter: 3,
        },
        Event::TaskCreated {
            title: "Seal the greenhouse frame".into(),
            fields: InitialFields::default(),
        },
    )
    .unwrap()
    .encode()
    .unwrap();
    let mut frame: Cbor = ciborium::from_reader(creation.as_slice()).unwrap();
    let (_, event) = frame.as_array_mut().unwrap()[1]
        .as_map_mut()
        .unwrap()
        .iter_mut()
        .find(|(key, _)| key.as_text() == Some("event"))
        .unwrap();
    let (_, title) = event.as_map_mut().unwrap()[0]
        .1
        .as_map_mut()
        .unwrap()
        .iter_mut()
        .find(|(key, _)| key.as_text() == Some("title"))
        .unwrap();
    *title = Cbor::from("");
    let mut bytes = Vec::new();
    ciborium::into_writer(&frame, &mut bytes).unwrap();
    let board = app.local_state().board_id;
    let pepino_store::AppendOutcome::Skipped { id, .. } = store
        .append(board, &bytes, IdSource::Content)
        .await
        .unwrap()
    else {
        panic!("expected a skipped invalid task creation");
    };

    let Response::Log { events, titles, .. } = handle(&mut app, json!({"op":"log"})).await.unwrap()
    else {
        panic!("expected log");
    };
    let id = id.to_string();
    let change = serde_json::to_value(&events[0].change).unwrap();
    assert_eq!(change["task"], id);
    assert_eq!(titles.get(&id), Some(&None));
}

#[tokio::test]
async fn titles_are_normalized_on_creation_and_edit() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    let (_, id, tasks, _) = write(
        &mut app,
        json!({
            "op":"add_task","title":"  Replace the greenhouse latch  "
        }),
    )
    .await;
    let id = id.unwrap();
    assert_eq!(tasks[0].title, "Replace the greenhouse latch");
    let edit = |title| {
        json!({
            "op":"set_fields","task":id,"changes":{"title":{"type":"text","value":title}}
        })
    };
    assert!(
        !write(&mut app, edit(" Replace the greenhouse latch "))
            .await
            .0
    );
    for bad in ["  ", "Oil the hinges\nCheck the latch"] {
        for request in [json!({"op":"add_task","title":bad}), edit(bad)] {
            let error = handle(&mut app, request).await.unwrap_err();
            assert_eq!(error.kind().code(), "invalid_input");
            assert!(error.to_string().contains(&format!("{bad:?}")));
        }
    }
}

#[tokio::test]
async fn deletion_returns_the_prior_record_for_write_rendering() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    let id = add(&mut app, "Order the replacement latch").await;
    let prior = get(&mut app, &id).await.summary;
    let Response::Write {
        changed,
        tasks,
        deleted,
        names,
        ..
    } = handle(&mut app, json!({"op":"delete","task":id}))
        .await
        .unwrap()
    else {
        panic!("expected write response");
    };
    assert!(changed && tasks.is_empty());
    assert_eq!(names[&prior.author], app.local_state().device_name);
    assert_eq!(deleted, [prior]);
}

#[tokio::test]
async fn task_validation_reports_the_offending_values() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    for (request, value) in [
        (json!({"op":"get_task","task":"latch"}), "latch"),
        (
            json!({"op":"add_task","title":"Repair the latch","tags":["Hardware"]}),
            "Hardware",
        ),
        (
            json!({"op":"add_task","title":"Repair the latch","assignees":["Ada"]}),
            "Ada",
        ),
    ] {
        let error = handle(&mut app, request).await.unwrap_err();
        assert_eq!(error.kind().code(), "invalid_input");
        assert!(error.to_string().contains(&format!("{value:?}")));
    }
}

#[tokio::test]
async fn replacing_a_parent_reports_the_newly_excluded_edge() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    let latch = add(&mut app, "Install a new latch").await;
    let greenhouse = add(&mut app, "Renovate the greenhouse").await;
    let door = add(&mut app, "Rebuild the door").await;
    let link = |to: &str| json!({"op":"link","from":latch,"kind":"parent","to":to});

    assert_eq!(write(&mut app, link(&greenhouse)).await.3.len(), 1);
    let (changed, _, _, edges) = write(&mut app, link(&door)).await;
    assert!(changed);
    assert_eq!(edges.len(), 2);
    assert_eq!(edges[0].to, door);
    assert!(edges[0].effective);
    assert_eq!(edges[1].to, greenhouse);
    assert_eq!(edges[1].reason, Some(EdgeExclusionReason::SupersededParent));
    assert_eq!(get(&mut app, &latch).await.parent, Some(door.clone()));
    assert!(write(&mut app, link(&door)).await.3.is_empty());
}
