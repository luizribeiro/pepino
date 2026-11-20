use std::{
    collections::{HashMap, HashSet},
    os::unix::process::ExitStatusExt,
    path::PathBuf,
    process::Command,
};

use pepino_core::BoardId;
use pepino_node::Role;

use super::support::{WAIT, enroll, network, next, settings, start};

const EVENTS: usize = 24;

fn acknowledged(index: usize, mask: u32) -> bool {
    index < 3 || (index >= 6 && mask & (1 << index) != 0)
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn abort_recovers_exactly_the_random_unacknowledged_subset() {
    let dir = tempfile::tempdir().unwrap();
    let network = network(dir.path());
    let owner_dir = dir.path().join("alice");
    let config = settings(&network, owner_dir.clone());
    let a = start(&config).await;
    let b = start(&settings(&network, dir.path().join("ben"))).await;
    let c = start(&settings(&network, dir.path().join("chloe"))).await;
    let mut owner = a.create_board().await.unwrap();
    let other = enroll(&owner, &b, Role::Write).await;
    let third = enroll(&owner, &c, Role::Write).await;
    let board_id = owner.id();
    let mut events = owner.events().unwrap();
    let mut sent = HashMap::new();
    for index in 0..EVENTS {
        let writer = [&owner, &other, &third][index % 3];
        let id = writer
            .publish(index.to_string().into_bytes())
            .await
            .unwrap();
        sent.insert(id, index);
    }
    let mut delivered = HashSet::new();
    while delivered.len() < EVENTS {
        let event = next(&mut events).await;
        assert!(sent.contains_key(&event.id));
        delivered.insert(event.id);
    }
    c.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    a.shutdown().await.unwrap();

    let random = p2panda::Topic::random();
    let mask = u32::from_le_bytes(random.as_bytes()[..4].try_into().unwrap());
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "crash::abort_after_acknowledging"])
        .current_dir(dir.path())
        .env("PEPINO_CRASH_DIR", &owner_dir)
        .env("PEPINO_CRASH_NETWORK", &network)
        .env("PEPINO_CRASH_BOARD", board_id.to_string())
        .env("PEPINO_CRASH_MASK", mask.to_string())
        .output()
        .unwrap();
    assert_eq!(child.status.signal(), Some(6), "{child:?}");

    let a = start(&config).await;
    let mut board = a.open_board(board_id).await.unwrap();
    let mut events = board.events().unwrap();
    let mut expected: HashMap<_, _> = sent
        .into_iter()
        .filter(|(_, i)| !acknowledged(*i, mask))
        .collect();
    let mut last = [None; 3];
    while !expected.is_empty() {
        let event = next(&mut events).await;
        let index = expected
            .remove(&event.id)
            .unwrap_or_else(|| panic!("unexpected event {} for mask {mask:#x}", event.id));
        assert_eq!(event.bytes, index.to_string().into_bytes());
        assert!(last[index % 3].is_none_or(|previous| index > previous));
        last[index % 3] = Some(index);
        event.ack().await.unwrap();
    }
    board.wait_until_joined(WAIT).await.unwrap();
    board.wait_until_replayed(WAIT).await.unwrap();
    let sentinel = board
        .publish(b"Verify restart recovery".to_vec())
        .await
        .unwrap();
    assert_eq!(next(&mut events).await.id, sentinel);
    a.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "subprocess used by the crash recovery test"]
async fn abort_after_acknowledging() {
    let Ok(dir) = std::env::var("PEPINO_CRASH_DIR") else {
        return;
    };
    let network = std::env::var("PEPINO_CRASH_NETWORK").unwrap();
    let board_id: BoardId = std::env::var("PEPINO_CRASH_BOARD")
        .unwrap()
        .parse()
        .unwrap();
    let mask: u32 = std::env::var("PEPINO_CRASH_MASK").unwrap().parse().unwrap();
    let node = start(&settings(&network, PathBuf::from(dir))).await;
    let mut board = node.open_board(board_id).await.unwrap();
    let mut events = board.events().unwrap();
    let mut selected = Vec::new();
    for _ in 0..EVENTS {
        let event = next(&mut events).await;
        let index = std::str::from_utf8(&event.bytes)
            .unwrap()
            .parse::<usize>()
            .unwrap();
        if acknowledged(index, mask) {
            selected.push((index, event));
        }
    }
    selected.sort_unstable_by_key(|(index, _)| std::cmp::Reverse(*index));
    for (_, event) in selected {
        event.ack().await.unwrap();
    }
    std::process::abort();
}
