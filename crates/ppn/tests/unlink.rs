mod common;
use common::*;

#[test]
fn unlink_removes_relationships_in_sentence_order() {
    let (sandbox, _) = task_sandbox();
    let p = small_project(&sandbox);
    let task = &p.downstream[..8];
    let human = sandbox.run(&["unlink", task, "blocked-by", &p.blocker]);
    assert!(human.starts_with("Unlinked "));
    assert_eq!(
        show_task(&sandbox, task)["blocked_by"],
        serde_json::json!([])
    );

    let json = as_json(&sandbox.run(&["unlink", &p.blocker, "blocks", task, "--json"]));
    assert_write(&json, false);
    assert_eq!(json["edges"], serde_json::json!([]));
    assert!(
        sandbox
            .run(&["unlink", task, "blocked-by", &p.blocker])
            .starts_with("Already unlinked ")
    );

    let json = as_json(&sandbox.run(&["unlink", task, "parent", &p.parent, "--json"]));
    assert_write(&json, true);
    assert_eq!(json["edges"][0]["effective"], false);
    assert!(json["edges"][0].get("reason").is_none());
    assert!(show_task(&sandbox, task)["parent"].is_null());

    sandbox.run(&["link", &p.parent, "relates", task]);
    assert_eq!(show_task(&sandbox, task)["relates"][0], p.parent);
    sandbox.run(&["unlink", task, "relates", &p.parent]);
    assert_eq!(show_task(&sandbox, task)["relates"], serde_json::json!([]));
}

#[test]
fn unlink_validates_both_endpoints() {
    let (sandbox, data) = task_sandbox();
    let p = small_project(&sandbox);
    assert_task_reference_errors(&sandbox, &data, |bad| {
        sandbox
            .command()
            .args(["unlink", &p.parent, "relates", bad, "--json"])
            .output()
            .unwrap()
    });
    assert_eq!(
        show_task(&sandbox, &p.parent)["relates"],
        serde_json::json!([])
    );
}

fn append_remote(data: &std::path::Path, event: pepino_core::Event) {
    use pepino_core::{Clock, DeviceId, Envelope};
    use pepino_store::{IdSource, Store};

    seed_tasks(data, async |app| {
        let board = app.local_state().board_id;
        let store = Store::open(data.join("pepino.sqlite")).await.unwrap();
        let mut clock = Clock::from_hlc(store.latest_hlc(board).await.unwrap().unwrap());
        let event = Envelope::new(
            DeviceId::from_bytes([23; 32]),
            clock.tick(SEED_NOW).unwrap(),
            event,
        )
        .unwrap();
        store
            .append(board, &event.encode().unwrap(), IdSource::Content)
            .await
            .unwrap();
    });
}

#[test]
fn unlink_cleans_up_a_link_to_a_deleted_task() {
    let (sandbox, data) = task_sandbox();
    let p = small_project(&sandbox);
    append_remote(
        &data,
        pepino_core::Event::TaskDeleted {
            task: p.blocker.parse().unwrap(),
        },
    );
    let human = sandbox.run(&["unlink", &p.downstream, "blocked-by", &p.blocker[..8]]);
    assert!(human.starts_with("Unlinked "));
    assert!(human.contains("(deleted task)"));
    assert_eq!(
        as_json(&sandbox.run(&["show", &p.downstream, "--json"]))["edges"],
        serde_json::json!([])
    );
}

#[test]
fn unlink_reports_a_link_activated_by_the_write() {
    for json in [false, true] {
        let (sandbox, data) = task_sandbox();
        let p = small_project(&sandbox);
        append_remote(
            &data,
            pepino_core::Event::EdgeAdded {
                from: p.downstream.parse().unwrap(),
                kind: pepino_core::EdgeKind::Blocks,
                to: p.blocker.parse().unwrap(),
            },
        );
        let mut args = vec!["unlink", &p.blocker, "blocks", &p.downstream];
        if json {
            args.push("--json");
        }
        let output = sandbox.run(&args);
        if json {
            let output = as_json(&output);
            assert_eq!(
                output["activated"],
                serde_json::json!([{
                    "from": &p.downstream, "kind": "blocks", "to": &p.blocker, "effective": true
                }])
            );
        } else {
            assert!(output.contains(&format!(
                "Activated {} blocks {}",
                &p.downstream[..6],
                &p.blocker[..6]
            )));
        }
    }
}
