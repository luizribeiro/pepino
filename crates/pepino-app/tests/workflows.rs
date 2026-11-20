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
