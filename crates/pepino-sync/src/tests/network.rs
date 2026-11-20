mod fixture;

use fixture::pair;

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn enrolled_devices_share_a_board_and_keep_distinct_credentials() {
    let (_dir, a, b) = pair().await;
    assert_eq!(a.board(), b.board());
    assert_ne!(
        a.settings.credentials.device_id(),
        b.settings.credentials.device_id()
    );
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn freshly_enrolled_engines_report_idle_and_shut_down() {
    let (_dir, a, b) = pair().await;
    let (left, right) = tokio::join!(a.start(), b.start());
    tokio::join!(a.await_events(&left, 0), b.await_events(&right, 0));
    fixture::views_match(&a, &b).await;
    tokio::join!(left.stop(), right.stop());
}

use serde_json::json;

use super::directory::write;
use fixture::{WAIT, views_match};

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn publishes_an_offline_project_and_syncs_edits_back() {
    let (_dir, mut a, mut b) = pair().await;
    let pepino_app::Response::Write {
        created: Some(project),
        ..
    } = write(
        &mut a.app,
        json!({"op":"add_task","title":"Repair greenhouse","tags":["garden"]}),
    )
    .await
    else {
        panic!("expected a project ID");
    };
    write(
        &mut a.app,
        json!({"op":"add_task","title":"Replace the glazing","parent":project}),
    )
    .await;
    write(
        &mut a.app,
        json!({"op":"comment","task":project,"text":"Order safety glass"}),
    )
    .await;

    let (left, right) = tokio::join!(a.start(), b.start());
    let original = a.app.store().statistics(a.board()).await.unwrap().total;
    left.engine.wait_idle(WAIT).await.unwrap();
    b.await_events(&right, original).await;
    views_match(&a, &b).await;

    write(
        &mut b.app,
        json!({"op":"comment","task":project,"text":"Glass has arrived"}),
    )
    .await;
    right.engine.nudge();
    a.await_events(&left, original + 1).await;
    views_match(&a, &b).await;
    tokio::join!(left.stop(), right.stop());
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn concurrent_edits_to_one_task_converge() {
    let (_dir, mut a, mut b) = pair().await;
    let pepino_app::Response::Write {
        created: Some(task),
        ..
    } = write(
        &mut a.app,
        json!({"op":"add_task","title":"Repair the greenhouse door"}),
    )
    .await
    else {
        panic!("expected a task ID");
    };
    let (left, right) = tokio::join!(a.start(), b.start());
    b.await_events(&right, 1).await;
    tokio::join!(left.stop(), right.stop());
    for (app, state, comment) in [
        (&mut a.app, "doing", "Measured the frame"),
        (&mut b.app, "done", "Fitted the spare latch"),
    ] {
        write(app, json!({"op":"set_fields","task":task,"changes":{"status":{"type":"status","value":state}}})).await;
        write(app, json!({"op":"comment","task":task,"text":comment})).await;
    }
    let (left, right) = tokio::join!(a.start(), b.start());
    tokio::join!(a.await_events(&left, 5), b.await_events(&right, 5));
    views_match(&a, &b).await;
    tokio::join!(left.stop(), right.stop());
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn a_returning_peer_and_a_late_member_both_catch_up() {
    let (dir, mut a, b) = pair().await;
    let (left, right) = tokio::join!(a.start(), b.start());
    write(
        &mut a.app,
        json!({"op":"add_task","title":"Inventory greenhouse tools"}),
    )
    .await;
    left.engine.nudge();
    b.await_events(&right, 1).await;
    right.stop().await;

    for title in [
        "Oil the shears",
        "Sharpen the spade",
        "Replace the broken rake",
    ] {
        write(&mut a.app, json!({"op":"add_task","title":title})).await;
    }
    left.engine.wait_idle(WAIT).await.unwrap();
    let right = b.start().await;
    b.await_events(&right, 4).await;
    views_match(&a, &b).await;

    left.stop().await;
    let c = fixture::enroll(dir.path(), &a, "chloe").await;
    let (left, third) = tokio::join!(a.start(), c.start());
    c.await_events(&third, 4).await;
    views_match(&a, &c).await;
    tokio::join!(left.stop(), right.stop(), third.stop());
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn a_forged_transport_author_is_logged_without_changing_the_view() {
    use pepino_core::{Envelope, Event, Hlc, TaskId};
    use pepino_node::Node;
    use pepino_store::EventClassification;

    let (_dir, mut a, b) = pair().await;
    let pepino_app::Response::Write {
        created: Some(task),
        ..
    } = write(
        &mut a.app,
        json!({"op":"add_task","title":"Fix the shed roof"}),
    )
    .await
    else {
        panic!("expected a task ID");
    };
    let (left, right) = tokio::join!(a.start(), b.start());
    b.await_events(&right, 1).await;
    let before = b.app.store().board_with_comments(b.board()).await.unwrap();
    left.stop().await;

    let raw = Node::spawn(a.settings.clone()).await.unwrap();
    let board = raw.open_board(a.board()).await.unwrap();
    board.wait_until_joined(WAIT).await.unwrap();
    board.wait_until_replayed(WAIT).await.unwrap();
    let forged = Envelope::new(
        b.app.local_state().device_id,
        Hlc {
            wall_ms: super::directory::NOW + 1,
            counter: 0,
        },
        Event::TaskDeleted {
            task: task.parse::<TaskId>().unwrap(),
        },
    )
    .unwrap();
    board.publish(forged.encode().unwrap()).await.unwrap();
    b.await_events(&right, 2).await;
    views_match(&a, &b).await;
    let history = b.app.store().history(b.board(), None, None).await.unwrap();
    assert!(
        history
            .events
            .iter()
            .any(|entry| entry.decoded.id() == forged.id()
                && entry.classification == EventClassification::ForgedAuthor)
    );
    assert_eq!(
        before,
        b.app.store().board_with_comments(b.board()).await.unwrap()
    );
    assert_eq!(
        b.app
            .store()
            .outbox(
                b.board(),
                b.app.local_state().device_id,
                1.try_into().unwrap()
            )
            .await
            .unwrap()
            .pending,
        0
    );
    board.close().await.unwrap();
    tokio::try_join!(raw.shutdown(), async {
        right.stop().await;
        Ok(())
    })
    .unwrap();
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn restores_a_replaced_event_store_with_its_own_and_peer_events() {
    let (dir, mut a, mut b) = pair().await;
    for (device, title) in [(&mut a, "Label shelves"), (&mut b, "Sort seed packets")] {
        write(&mut device.app, json!({"op":"add_task","title":title})).await;
    }
    let (left, right) = tokio::join!(a.start(), b.start());
    tokio::join!(a.await_events(&left, 2), b.await_events(&right, 2));
    views_match(&a, &b).await;
    tokio::join!(left.stop(), right.stop());

    let board = b.board();
    let device = b.app.local_state().device_id;
    drop(b.app);
    let path = dir.path().join("ben");
    std::fs::remove_file(path.join("pepino.sqlite")).unwrap();
    b.app = super::directory::prepare(&path, board, device).await;
    assert_eq!(b.app.store().statistics(board).await.unwrap().total, 0);

    let right = b.start().await;
    b.await_events(&right, 2).await;
    views_match(&a, &b).await;
    right.stop().await;
}
