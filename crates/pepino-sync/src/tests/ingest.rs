use std::{sync::Arc, time::Duration};

use pepino_core::{BoardId, DeviceId};
use serde_json::json;

use super::{WAIT, directory, stub::Stub};
use crate::SyncEngine;

#[tokio::test]
async fn incompatible_shared_board_cards_report_an_upgrade_before_acknowledging() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([15; 32]);
    let local = DeviceId::from_bytes([16; 32]);
    let peer = DeviceId::from_bytes([17; 32]);
    let app = directory::prepare(dir.path(), board, local).await;
    let node = Arc::new(Stub::new(local));
    let engine = SyncEngine::start(Default::default(), node.clone(), dir.path())
        .await
        .unwrap();
    let version = pepino_app::PROTOCOL_VERSION;
    for other_version in [1, version + 1] {
        let body = pepino_app::WireFormat::Cbor
            .encode(&json!({"v":other_version,"cards":{"membership":null}}))
            .unwrap();
        let bytes = [b"pepino-board-people\0".as_slice(), &body].concat();
        let mut receipt = node.deliver(board, peer, bytes);
        tokio::time::timeout(WAIT, receipt.wait_for(|done| *done))
            .await
            .unwrap()
            .unwrap();
        let status = engine.status();
        let error = status.boards[&board].last_error.as_deref().unwrap();
        assert!(error.contains(&format!("protocol version {other_version} is unsupported")));
        assert!(error.contains("Upgrade the older pepino or client"));
        assert_eq!(app.store().statistics(board).await.unwrap().total, 0);
    }
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn malformed_payloads_are_retained_before_acknowledging_and_ingest_continues() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([15; 32]);
    let local = DeviceId::from_bytes([16; 32]);
    let peer = DeviceId::from_bytes([17; 32]);
    let app = directory::prepare(&dir.path().join("left"), board, local).await;
    let mut sender = directory::prepare(&dir.path().join("right"), board, peer).await;
    directory::write(
        &mut sender,
        json!({"op":"add_task","title":"Install rain barrel"}),
    )
    .await;
    let node = Arc::new(Stub::new(local));
    let engine = SyncEngine::start(Default::default(), node.clone(), dir.path().join("left"))
        .await
        .unwrap();
    let bytes = sender
        .store()
        .outbox(board, peer, 1.try_into().unwrap())
        .await
        .unwrap()
        .events
        .remove(0)
        .bytes;
    let malformed = vec![0xff];
    for (payload, total) in [(malformed.clone(), 1), (bytes, 2)] {
        let mut receipt = node.deliver(board, peer, payload);
        tokio::time::timeout(WAIT, receipt.wait_for(|done| *done))
            .await
            .unwrap()
            .unwrap();
        let stats = app.store().statistics(board).await.unwrap();
        assert_eq!((stats.total, stats.undecodable), (total, 1));
        assert_eq!(
            app.store().undecodable(board).await.unwrap(),
            [(peer, malformed.clone())]
        );
    }
    assert_eq!(
        app.store().tasks(board).await.unwrap()[0].title,
        "Install rain barrel"
    );
    assert!(
        engine.status().boards[&board]
            .last_error
            .as_ref()
            .unwrap()
            .contains("invalid event CBOR")
    );
    let snapshot = engine.status();
    assert_eq!(snapshot.boards[&board].received, 1);
    assert!(snapshot.boards[&board].last_received_at.is_some());
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn waits_for_the_local_lock_before_acknowledging() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([21; 32]);
    let device = DeviceId::from_bytes([22; 32]);
    let app = directory::prepare(dir.path(), board, device).await;
    let node = Arc::new(Stub::new(device));
    let engine = SyncEngine::start(
        crate::Settings {
            lock_timeout: Duration::ZERO,
            ..Default::default()
        },
        node.clone(),
        dir.path(),
    )
    .await
    .unwrap();
    let lock = std::fs::File::options()
        .write(true)
        .open(dir.path().join("local.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let mut receipt = node.deliver(board, device, vec![0x82, 2, 0xf6]);
    let busy = super::wait_for_status(&engine, |status| {
        status.boards[&board]
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("holds the data directory"))
    })
    .await;
    assert!(busy.boards[&board].ingest_pending);
    assert!(!*receipt.borrow());
    assert_eq!(app.store().statistics(board).await.unwrap().total, 0);
    drop(lock);
    tokio::time::timeout(WAIT, receipt.wait_for(|done| *done))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        app.store().statistics(board).await.unwrap().unknown_version,
        1
    );
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_crash_after_commit_redelivers_as_a_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([44; 32]);
    let local = DeviceId::from_bytes([45; 32]);
    let peer = DeviceId::from_bytes([46; 32]);
    let app = directory::prepare(dir.path(), board, local).await;
    let mut remote = directory::prepare(&dir.path().join("peer"), board, peer).await;
    directory::write(
        &mut remote,
        json!({"op":"add_task","title":"Repair the awning"}),
    )
    .await;
    let bytes = remote
        .store()
        .outbox(board, peer, 1.try_into().unwrap())
        .await
        .unwrap()
        .events
        .remove(0)
        .bytes;
    let node = Arc::new(Stub::new(local));
    let engine = SyncEngine::start(Default::default(), node.clone(), dir.path())
        .await
        .unwrap();
    let (mut attempting, receipt) = node.pause_before_ack(board, peer, bytes.clone());
    tokio::time::timeout(WAIT, attempting.wait_for(|started| *started))
        .await
        .unwrap()
        .unwrap();
    assert!(!*receipt.borrow());
    let before = app.store().board_with_comments(board).await.unwrap();
    assert_eq!(before.tasks()[0].title, "Repair the awning");
    {
        let idle = engine.sync_once();
        tokio::pin!(idle);
        assert!(futures_util::poll!(&mut idle).is_pending());
    }
    drop(engine);

    let restarted = Arc::new(Stub::new(local));
    let engine = SyncEngine::start(Default::default(), restarted.clone(), dir.path())
        .await
        .unwrap();
    let mut redelivered = restarted.deliver(board, peer, bytes);
    tokio::time::timeout(WAIT, redelivered.wait_for(|done| *done))
        .await
        .unwrap()
        .unwrap();
    engine.wait_idle(WAIT).await.unwrap();
    assert_eq!(
        app.store().board_with_comments(board).await.unwrap(),
        before
    );
    assert_eq!(app.store().statistics(board).await.unwrap().total, 1);
    assert_eq!(engine.status().boards[&board].last_ingested, None);
    assert!(restarted.published().is_empty());
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn waits_for_a_connected_peers_first_pull_and_ingests_its_events() {
    use pepino_core::{Envelope, Event, Hlc, InitialFields};
    use std::task::Poll;

    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([61; 32]);
    let local = DeviceId::from_bytes([62; 32]);
    let peer = DeviceId::from_bytes([63; 32]);
    let app = directory::prepare(dir.path(), board, local).await;
    let node = Arc::new(Stub::new(local));
    let engine = SyncEngine::start(Default::default(), node.clone(), dir.path())
        .await
        .unwrap();
    let limit = WAIT;
    engine.wait_idle(limit).await.unwrap();
    assert!(engine.status().is_idle());
    assert!(engine.status().node.connected_peers.is_empty());
    node.connect(peer);

    {
        let idle = engine.wait_idle(limit);
        tokio::pin!(idle);
        assert_eq!(futures_util::poll!(&mut idle).map(|_| ()), Poll::Pending);
        super::wait_for_status(&engine, |status| {
            status.boards[&board].scan_generation == Some(2)
        })
        .await;
        assert!(!engine.status().is_idle());
        assert_eq!(futures_util::poll!(&mut idle).map(|_| ()), Poll::Pending);
        let event = Envelope::new(
            peer,
            Hlc {
                wall_ms: 100,
                counter: 0,
            },
            Event::TaskCreated {
                title: "Count the seed trays".into(),
                fields: InitialFields::default(),
            },
        )
        .unwrap();
        let ack = node.deliver(board, peer, event.encode().unwrap());
        node.caught_up(board, peer);
        idle.await.unwrap();
        assert!(*ack.borrow());
    }
    assert_eq!(
        app.store().tasks(board).await.unwrap()[0].title,
        "Count the seed trays"
    );
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn stream_errors_back_off_and_keep_the_board_non_idle_until_repoll() {
    use futures_util::{StreamExt, stream};
    use std::time::Duration;
    use tokio::{sync::watch, time::Instant};

    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([65; 32]);
    let app = directory::prepare(dir.path(), board, DeviceId::from_bytes([66; 32])).await;
    use crate::Transport;
    let node = Stub::new(DeviceId::from_bytes([66; 32]));
    let handle = node.open_board(board).await.unwrap();
    tokio::time::pause();
    let mut idle = crate::Status::default();
    idle.boards.entry(board).or_default().outbox_pending = Some(0);
    idle.node.boards.insert(board, Default::default());
    let (status, _) = watch::channel(idle);
    let (polled, mut poll_times) = tokio::sync::mpsc::unbounded_channel();
    let attempt = |error: &str| {
        Err::<crate::Delivery<super::stub::Receipt>, _>(pepino_node::Error::Node(error.into()))
    };
    let stream = stream::iter([attempt("interrupted"), attempt("still interrupted")])
        .map(move |item| {
            polled.send(Instant::now()).unwrap();
            item
        })
        .chain(stream::pending())
        .boxed();
    let (started, mut states) = watch::channel(false);
    let observe = async {
        let first = poll_times.recv().await.unwrap();
        started.send_replace(true);
        let second = poll_times.recv().await.unwrap();
        assert!(second - first >= Duration::from_millis(25));
    };
    let mut updates = status.subscribe();
    let seq = async {
        states.wait_for(|started| *started).await.unwrap();
        let retrying = updates
            .wait_for(|status| status.boards[&board].stream_retrying)
            .await
            .unwrap();
        assert!(!retrying.is_idle());
        drop(retrying);
        let idle = updates.wait_for(crate::Status::is_idle).await.unwrap();
        assert!(
            idle.boards[&board]
                .last_error
                .as_deref()
                .unwrap()
                .contains("still interrupted")
        );
    };
    let (_stop, stopping) = watch::channel(());
    tokio::select! {
        result = crate::inbox::run(&app, board, &handle, stream, &status, stopping, None) => panic!("{result:?}"),
        _ = async { tokio::join!(observe, seq); } => (),
    }
}

#[tokio::test]
async fn retries_ack_without_appending_again() {
    use super::stub::Receipt;
    use pepino_core::EventId;

    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([74; 32]);
    let device = DeviceId::from_bytes([75; 32]);
    let app = directory::prepare(dir.path(), board, device).await;
    let node = Arc::new(Stub::new(device));
    let engine = SyncEngine::start(Default::default(), node.clone(), dir.path())
        .await
        .unwrap();
    let ack = Receipt::new();
    node.deliver_operation(
        board,
        EventId::from_bytes([76; 32]),
        device,
        vec![0x82, 2, 0xf6],
        ack.clone().with_failures(2),
    );
    super::wait_for_status(&engine, |s| {
        s.boards[&board]
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("test acknowledgement failure"))
    })
    .await;
    assert_eq!(
        app.store().statistics(board).await.unwrap().unknown_version,
        1
    );
    engine.wait_idle(WAIT).await.unwrap();
    assert!(ack.is_acked());
    assert_eq!(app.store().statistics(board).await.unwrap().total, 1);
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_commits_and_acknowledges_the_delivery_it_already_took() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([74; 32]);
    let device = DeviceId::from_bytes([75; 32]);
    let app = directory::prepare(dir.path(), board, device).await;
    let node = Arc::new(Stub::new(device));
    let engine = SyncEngine::start(
        crate::Settings {
            lock_timeout: Duration::ZERO,
            ..Default::default()
        },
        node.clone(),
        dir.path(),
    )
    .await
    .unwrap();
    let lock = std::fs::File::open(dir.path().join("local.lock")).unwrap();
    lock.lock().unwrap();
    let ack = node.deliver(board, device, vec![0x82, 2, 0xf6]);
    super::wait_for_status(&engine, |status| {
        status.boards[&board]
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("holds the data directory"))
    })
    .await;
    let queued = node.deliver(board, device, vec![0x82, 2, 0xf5]);
    // SQLite uses blocking I/O and can outlive the test's virtual retry interval.
    tokio::time::pause();
    let (release_clock, hold_clock) = std::sync::mpsc::channel::<()>();
    let clock = tokio::task::spawn_blocking(move || hold_clock.recv_timeout(WAIT));
    let shutdown = engine.shutdown();
    tokio::pin!(shutdown);
    tokio::select! {
        biased;
        result = &mut shutdown => panic!("shutdown completed while the writer was locked: {result:?}"),
        _ = std::future::ready(()) => (),
    }
    drop(lock);
    tokio::time::advance(Duration::from_millis(100)).await;
    let result = shutdown.await;
    drop(release_clock);
    assert_eq!(
        clock.await.unwrap(),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    );
    tokio::time::resume();
    result.unwrap();
    assert!(*ack.borrow());
    assert!(!*queued.borrow());
    assert_eq!(
        app.store().statistics(board).await.unwrap().unknown_version,
        1
    );
}

#[tokio::test]
async fn a_pending_leave_pauses_incoming_task_updates() {
    use pepino_app::{App, DeviceGrant, OpenOptions, PeerHint, SignedGrant};
    use pepino_node::{Node, NodeSettings};

    let dir = tempfile::tempdir().unwrap();
    let laptop = dir.path().join("laptop");
    let tablet = dir.path().join("tablet");
    let root = App::init(&laptop, directory::NOW).await.unwrap();
    let keys = root.identity_credentials().unwrap();
    let peer = keys.device_id();
    let request = App::request_device(
        &tablet,
        OpenOptions {
            device_name: Some("Greenhouse tablet".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let local = request.id;
    root.remember_device(local, request.name.clone(), None)
        .await
        .unwrap();
    let roster = root.fresh_local_state().await.unwrap();
    let board = roster.catalog_board();
    let grant = DeviceGrant {
        recipient: local,
        request: request.binding(),
        identity: roster.identity_id,
        display_name: roster.display_name().into(),
        boards: roster.boards().to_vec(),
        devices: roster.devices(),
        peer: PeerHint {
            id: peer,
            relay_url: None,
        },
    };
    App::join_device(
        &tablet,
        SignedGrant::issue(&grant, &request, &keys).unwrap(),
        Default::default(),
    )
    .await
    .unwrap();
    let app = App::open(&tablet, directory::NOW).await.unwrap();
    app.reconcile_identity(peer, &app.identity_credentials().unwrap())
        .await
        .unwrap();
    let node = Node::spawn(NodeSettings::offline(keys, laptop.join("node.sqlite")))
        .await
        .unwrap();
    let member = node.member_info().await.unwrap();
    node.shutdown().await.unwrap();
    let catalog = roster.encode_catalog(&member).unwrap();
    assert!(
        app.receive_catalog(board, peer, &catalog, directory::NOW)
            .await
            .unwrap()
    );
    app.record_leave_request(std::time::SystemTime::now().into())
        .await
        .unwrap();
    let node = Arc::new(Stub::new(local));
    let engine = SyncEngine::start(Default::default(), node.clone(), &tablet)
        .await
        .unwrap();
    let mut receipt = node.deliver(board, peer, vec![0x82, 2, 0xf6]);
    tokio::time::timeout(WAIT, receipt.wait_for(|done| *done))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(app.store().statistics(board).await.unwrap().total, 0);
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn membership_authorization_is_retried_before_acknowledgment() {
    use pepino_app::{App, BoardCards, MembershipAction, MembershipChange};
    use pepino_node::Role;

    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let local = app.local_state();
    let board = BoardId::from_bytes([91; 32]);
    let manager = DeviceId::from_bytes([92; 32]);
    app.register_created_board(board, "Greenhouse")
        .await
        .unwrap();
    let node = Arc::new(Stub::new(local.device_id));
    let mut probes = node.watch_operation_roles();
    let engine = SyncEngine::start(Default::default(), node.clone(), dir.path())
        .await
        .unwrap();
    let payload = BoardCards::Membership(MembershipChange {
        person: local.own_person_key(),
        action: MembershipAction::Removed,
        at: std::time::UNIX_EPOCH.into(),
        revision: 1,
        incarnation: None,
        parents: Default::default(),
    })
    .encode()
    .unwrap();
    let mut received = node.deliver(board, manager, payload);
    tokio::time::timeout(WAIT, probes.wait_for(|queries| *queries >= 2))
        .await
        .unwrap()
        .unwrap();
    assert!(!*received.borrow());
    assert_eq!(
        app.fresh_local_state()
            .await
            .unwrap()
            .membership_history(board)
            .count(),
        0
    );
    node.set_joined_members(
        board,
        [(local.device_id, Role::Manage), (manager, Role::Manage)].into(),
    );
    node.set_operation_role(Role::Manage);
    tokio::time::timeout(WAIT, received.wait_for(|acked| *acked))
        .await
        .unwrap()
        .unwrap();
    let state = app.fresh_local_state().await.unwrap();
    let mut history = state.membership_history(board);
    assert_eq!(history.next().unwrap().refusal, None);
    assert!(history.next().is_none());
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_reader_updates_their_card_after_a_former_manager_sent_unrelated_routes() {
    use crate::Transport;
    use pepino_app::{
        App, BoardAdmission, BoardCards, BoardRole, MembershipAction, MembershipChange,
        MembershipRecord, Request, WriteRequest,
    };
    use pepino_core::EventId;
    use pepino_node::Role;

    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let local = app.local_state();
    let mut reader = App::init(dir.path().join("reader"), 0).await.unwrap();
    let reader_device = reader.local_state().device_id;
    let reader_key = reader.local_state().own_person_key();
    let invitation = EventId::from_bytes([98; 32]);
    let board = BoardId::from_bytes([93; 32]);
    let approved = BoardId::from_bytes([94; 32]);
    let declined = BoardId::from_bytes([95; 32]);
    let writer = DeviceId::from_bytes([96; 32]);
    app.register_created_board(board, "Greenhouse")
        .await
        .unwrap();
    let node = Arc::new(Stub::new(local.device_id));
    let engine = SyncEngine::start(Default::default(), node.clone(), dir.path())
        .await
        .unwrap();
    node.set_operation_role(Role::Manage);
    let person = (pepino_core::IdentityId::from_bytes([97; 32]), writer);
    node.set_joined_members(
        board,
        [(local.device_id, Role::Manage), (writer, Role::Manage)].into(),
    );
    for number in 0..160u8 {
        let mut bytes = [number; 32];
        bytes[31] = 249;
        let payload = BoardCards::ReaderRoute {
            channel: BoardId::from_bytes(bytes),
            person,
            incarnation: None,
        }
        .encode()
        .unwrap();
        let mut received = node.deliver(board, writer, payload);
        tokio::time::timeout(WAIT, received.wait_for(|acked| *acked))
            .await
            .unwrap()
            .unwrap();
    }
    node.set_joined_members(board, [(local.device_id, Role::Manage)].into());
    app.record_membership(
        board,
        MembershipRecord {
            operation: invitation,
            actor: local.device_id,
            change: MembershipChange {
                person: reader_key,
                action: MembershipAction::Invited(BoardRole::Read),
                at: std::time::UNIX_EPOCH.into(),
                revision: 1,
                incarnation: None,
                parents: Default::default(),
            },
            refusal: None,
            applied: true,
            relayed: false,
        },
    )
    .await
    .unwrap();
    app.record_board_admission(
        board,
        local.device_id,
        local.device_id,
        BoardAdmission {
            owner: local.identity_card().unwrap(),
            owner_device: local.device_id,
            invitee: reader.local_state().identity_card().unwrap(),
            device: reader_device,
            at: std::time::UNIX_EPOCH.into(),
        },
    )
    .await
    .unwrap();
    node.set_joined_members(
        board,
        [(local.device_id, Role::Manage), (reader_device, Role::Read)].into(),
    );
    for (signer, channel) in [(local.device_id, approved), (writer, declined)] {
        let payload = BoardCards::ReaderRoute {
            channel,
            person: reader_key,
            incarnation: Some(invitation),
        }
        .encode()
        .unwrap();
        let mut received = node.deliver(board, signer, payload);
        tokio::time::timeout(WAIT, received.wait_for(|acked| *acked))
            .await
            .unwrap()
            .unwrap();
    }
    let local = app.fresh_local_state().await.unwrap();
    assert!(local.card_exchanges().contains_key(&approved));
    assert!(!local.card_exchanges().contains_key(&declined));
    assert_eq!(local.card_exchanges().len(), 1);
    engine.nudge();
    super::until(
        async || {
            tokio::task::yield_now().await;
            node.status().boards.contains_key(&approved)
        },
        |&open| open,
        WAIT,
    )
    .await;
    reader
        .handle(
            Request::Write(WriteRequest::IdentityRename {
                name: "Greenhouse reader".into(),
            }),
            0,
        )
        .await
        .unwrap();
    let renamed = reader
        .fresh_local_state()
        .await
        .unwrap()
        .identity_card()
        .unwrap();
    let update = BoardCards::Update(renamed).encode().unwrap();
    let mut received = node.deliver(approved, reader_device, update);
    tokio::time::timeout(WAIT, received.wait_for(|acked| *acked))
        .await
        .unwrap()
        .unwrap();
    let local = app.fresh_local_state().await.unwrap();
    assert_eq!(
        local
            .board_people(board)
            .find(|person| person.key() == reader_key)
            .unwrap()
            .name(),
        "Greenhouse reader"
    );
    engine.shutdown().await.unwrap();
}
