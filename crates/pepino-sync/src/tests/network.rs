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
