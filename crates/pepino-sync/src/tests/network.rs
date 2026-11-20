mod fixture;

use fixture::pair;

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn enrolled_engines_share_a_board_keep_distinct_credentials_and_report_idle() {
    let (_dir, a, b) = pair().await;
    assert_eq!(a.board(), b.board());
    assert_ne!(
        a.settings.credentials.device_id(),
        b.settings.credentials.device_id()
    );
    let (left, right) = tokio::join!(a.start(), b.start());
    tokio::try_join!(left.engine.wait_idle(WAIT), right.engine.wait_idle(WAIT)).unwrap();
    fixture::views_match(&a, &b).await;
    tokio::join!(left.stop(), right.stop());
}

use serde_json::json;

use super::{WAIT, directory::write, until};
use fixture::views_match;

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
    left.engine.wait_idle(WAIT).await.unwrap();
    views_match(&a, &b).await;

    write(
        &mut b.app,
        json!({"op":"comment","task":project,"text":"Glass has arrived"}),
    )
    .await;
    right.engine.nudge();
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
    views_match(&a, &b).await;
    tokio::join!(left.stop(), right.stop());
    for (app, state, comment) in [
        (&mut a.app, "doing", "Measured the frame"),
        (&mut b.app, "done", "Fitted the spare latch"),
    ] {
        write(app, json!({"op":"set_fields","task":task,"changes":{"status":{"type":"status","value":state}}})).await;
        write(app, json!({"op":"comment","task":task,"text":comment})).await;
    }
    let (left, right) = tokio::join!(a.start(), b.start());
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
    views_match(&a, &b).await;
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
    views_match(&a, &b).await;

    left.stop().await;
    let c = fixture::enroll(dir.path(), &a, "chloe").await;
    let (left, third) = tokio::join!(a.start(), c.start());
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
    views_match(&a, &b).await;
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
    let mut ticks = tokio::time::interval(std::time::Duration::from_millis(25));
    until(
        async || {
            ticks.tick().await;
            b.app.store().history(b.board(), None, None).await.unwrap()
        },
        |history| {
            history.events.iter().any(|entry| {
                entry.decoded.id() == forged.id()
                    && entry.classification == EventClassification::ForgedAuthor
            })
        },
        WAIT,
    )
    .await;
    right.engine.wait_idle(WAIT).await.unwrap();
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
    views_match(&a, &b).await;
    right.stop().await;
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn the_running_engine_enrolls_a_device_on_every_board_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let owner = fixture::owner(dir.path()).await;
    let node = pepino_node::Node::spawn(owner.settings.clone())
        .await
        .unwrap();
    let workshop = node.create_board().await.unwrap();
    let workshop_id = workshop.id();
    workshop.close().await.unwrap();
    node.shutdown().await.unwrap();
    let path = dir.path().join("alice/local.json");
    let mut local = serde_json::to_value(owner.app.local_state()).unwrap();
    local["boards"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": workshop_id, "name": "Workshop"}));
    std::fs::write(path, local.to_string()).unwrap();
    let live = owner.start().await;
    let peer_dir = dir.path().join("ferrum");
    let peer = pepino_app::App::request_device(
        &peer_dir,
        pepino_app::OpenOptions {
            device_name: Some("Ferrum".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    live.engine.add_member(peer.clone(), None).await.unwrap();
    live.engine.add_member(peer.clone(), None).await.unwrap();
    let local = pepino_app::App::open(dir.path().join("alice"), 0)
        .await
        .unwrap();
    assert_eq!(local.local_state().devices()[&peer.id], "Ferrum");
    let admitted = &local.local_state().roster()[&peer.id];
    assert_eq!(admitted.added_by, Some(owner.app.local_state().device_id));
    assert!(admitted.added_at.is_some());
    let mut settings = owner.settings.clone();
    settings.credentials =
        pepino_node::DeviceCredentials::load(peer_dir.join("credentials.cbor")).unwrap();
    settings.database_path = peer_dir.join("node.sqlite");
    let node = pepino_node::Node::spawn(settings).await.unwrap();
    let first = node.open_board(owner.board()).await.unwrap();
    let second = node.open_board(workshop_id).await.unwrap();
    for board in [&first, &second] {
        board.wait_until_joined(WAIT).await.unwrap();
        assert!(
            board
                .members()
                .await
                .unwrap()
                .contains(&(peer.id, pepino_node::Role::Manage))
        );
    }
    first.close().await.unwrap();
    second.close().await.unwrap();
    node.shutdown().await.unwrap();
    live.stop().await;
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn enrollment_checks_manage_access_before_changing_membership() {
    let (dir, _, writer) = fixture::pair().await;
    let candidate = pepino_app::App::request_device(
        &dir.path().join("garden-tablet"),
        pepino_app::OpenOptions::default(),
    )
    .await
    .unwrap();
    let live = writer.start().await;
    let err = live
        .engine
        .add_member(candidate.clone(), None)
        .await
        .unwrap_err();
    let board_name = writer.app.local_state().board_name();
    assert!(matches!(err, crate::Error::EnrollmentAccess { board } if board == board_name));
    let refreshed = pepino_app::App::open(dir.path().join("ben"), 0)
        .await
        .unwrap();
    assert!(
        !refreshed
            .local_state()
            .devices()
            .contains_key(&candidate.id)
    );
    live.stop().await;
}
