use pepino_app::BoardJoinState;
use pepino_core::BoardId;
use tokio::runtime::{Builder, Runtime};

use super::super::protocol::{Operation, Outcome};
use super::{
    Daemon, Pair, Signal, WAIT, grant_membership, start_pair, until, verify_databases,
    wait_for_board_views, write_board_tasks,
};

fn linked_pair() -> (Runtime, Pair, Daemon, Daemon) {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();
    let (pair, owner, device) = start_pair(&rt);
    grant_membership(&pair, device);
    let peer = Daemon::start(pair.peer.path(), &pair.network);
    pair.joined();
    (rt, pair, owner, peer)
}

fn create_board(pair: &Pair, name: &str) -> BoardId {
    let reply = pair
        .client(pair.owner.path())
        .call_with(Operation::CreateBoard { name: name.into() }, WAIT)
        .unwrap();
    match reply {
        Outcome::BoardCreated { board } => board,
        other => panic!("create {name}: {other:?}"),
    }
}

#[test]
fn sigterm_during_runtime_board_discovery_recovers_tasks() {
    let (rt, pair, owner, peer) = linked_pair();
    let receiver = pair.client(pair.peer.path());
    let board = create_board(&pair, "Tool Shed");
    let _ = until(
        || receiver.status(),
        |status| {
            status
                .as_ref()
                .ok()
                .and_then(Option::as_ref)
                .is_some_and(|status| {
                    matches!(
                        status.network.board_joins.get(&board),
                        Some(
                            BoardJoinState::WaitingToBeAdded | BoardJoinState::WaitingForFirstSync
                        )
                    )
                })
        },
        WAIT,
    );
    peer.stop(Signal::SIGTERM);
    assert!(!receiver.path.exists());

    write_board_tasks(&rt, pair.owner.path(), Some("Tool Shed"), 0..3);
    pair.client(pair.owner.path()).nudge().unwrap();
    let peer = Daemon::start(pair.peer.path(), &pair.network);
    pair.board_joined(board);
    wait_for_board_views(&rt, &pair, board, 3);
    peer.stop(Signal::SIGTERM);
    owner.stop(Signal::SIGTERM);
    rt.block_on(verify_databases(&pair));
}

#[test]
fn sigterm_with_several_open_boards_recovers_tasks_on_each() {
    let (rt, pair, owner, peer) = linked_pair();
    let boards = [
        (None, pair.board),
        (Some("Tool Shed"), create_board(&pair, "Tool Shed")),
        (Some("Orchard"), create_board(&pair, "Orchard")),
    ];
    for (name, board) in boards {
        pair.board_joined(board);
        write_board_tasks(&rt, pair.owner.path(), name, 0..1);
        pair.client(pair.owner.path()).nudge().unwrap();
        wait_for_board_views(&rt, &pair, board, 1);
    }
    peer.stop(Signal::SIGTERM);
    assert!(!pair.client(pair.peer.path()).path.exists());
    for (name, _) in boards {
        write_board_tasks(&rt, pair.owner.path(), name, 1..3);
    }
    pair.client(pair.owner.path()).nudge().unwrap();
    let peer = Daemon::start(pair.peer.path(), &pair.network);
    for (_, board) in boards {
        wait_for_board_views(&rt, &pair, board, 3);
    }
    peer.stop(Signal::SIGTERM);
    owner.stop(Signal::SIGTERM);
    rt.block_on(verify_databases(&pair));
}
