use std::{
    collections::{HashMap, HashSet},
    net::Ipv6Addr,
    os::unix::process::ExitStatusExt,
    path::PathBuf,
    process::Command,
};

use p2panda_net::{addrs::NodeInfo, iroh_endpoint::EndpointAddr};
use p2panda_store::{Transaction, address_book::AddressBookStore, sqlite::SqliteStore};
use pepino_core::{BoardId, EventId};
use pepino_node::{BoardHandle, MdnsMode, Node, NodeSettings, Role};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode},
};

use super::support::{WAIT, enroll, fixture_dir, network, next, settings, start, until};

const EVENTS: usize = 24;

fn acknowledged(index: usize, mask: u32) -> bool {
    index < 3 || (index >= 6 && mask & (1 << index) != 0)
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn abort_recovers_exactly_the_random_unacknowledged_subset() {
    let dir = fixture_dir();
    let network = network(dir.path());
    let owner_dir = dir.path().join("alice");
    let config = local_settings(&network, owner_dir.clone());
    let ben = local_settings(&network, dir.path().join("ben"));
    let chloe = local_settings(&network, dir.path().join("chloe"));
    let a = start(&config).await;
    let mut owner = a.create_board().await.unwrap();
    let board_id = owner.id();
    let owner_info = owner_peer_info(&config, &a).await;
    let b = start_peer(&ben, &owner_info, board_id).await;
    let other = enroll(&owner, &b, Role::Write).await;
    let ticket = b.member_info().await.unwrap();
    assert_eq!(ticket.device_id().unwrap(), b.device_id());
    owner.add_member(&ticket, Role::Write).await.unwrap();
    assert!(matches!(
        owner.add_member(&ticket, Role::Read).await,
        Err(pepino_node::Error::Member(_))
    ));
    let c = start_peer(&chloe, &owner_info, board_id).await;
    let third = enroll(&owner, &c, Role::Write).await;

    let mut events = owner.events().unwrap();
    let (first, second, third) = tokio::join!(
        publish_history(&owner, 0),
        publish_history(&other, 1),
        publish_history(&third, 2),
    );
    let sent: HashMap<_, _> = first.into_iter().chain(second).chain(third).collect();
    let mut delivered = HashSet::new();
    while delivered.len() < EVENTS {
        let event = next(&mut events).await;
        assert!(sent.contains_key(&event.id));
        delivered.insert(event.id);
    }
    tokio::try_join!(a.shutdown(), b.shutdown(), c.shutdown()).unwrap();

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

fn local_settings(network: &str, dir: PathBuf) -> NodeSettings {
    let mut settings = settings(network, dir);
    settings.mdns = MdnsMode::Disabled;
    settings.bind_ip_v6 = Some(Ipv6Addr::LOCALHOST);
    settings
}

async fn address_book(settings: &NodeSettings, create: bool) -> SqlitePool {
    let options = SqliteConnectOptions::new()
        .filename(&settings.database_path)
        .create_if_missing(create)
        .journal_mode(SqliteJournalMode::Wal);
    SqlitePool::connect_with(options).await.unwrap()
}

async fn owner_peer_info(settings: &NodeSettings, owner: &Node) -> NodeInfo {
    let pool = address_book(settings, false).await;
    let store = SqliteStore::from_pool(pool.clone());
    let id = p2panda::VerifyingKey::from_bytes(owner.device_id().as_bytes()).unwrap();
    let endpoint = until(
        async || {
            AddressBookStore::<_, NodeInfo>::node_info(&store, &id)
                .await
                .unwrap()
                .and_then(|info| EndpointAddr::try_from(info).ok())
        },
        |endpoint| {
            endpoint
                .as_ref()
                .is_some_and(|value| value.ip_addrs().any(|addr| addr.ip().is_loopback()))
        },
        WAIT,
    )
    .await
    .unwrap();
    pool.close().await;
    let addrs = endpoint
        .ip_addrs()
        .copied()
        .filter(|addr| addr.ip().is_loopback());
    NodeInfo::from(addrs.fold(EndpointAddr::new(endpoint.id), EndpointAddr::with_ip_addr))
}

async fn start_peer(settings: &NodeSettings, owner: &NodeInfo, board: BoardId) -> Node {
    let pool = address_book(settings, true).await;
    p2panda_store::sqlite::run_pending_migrations(&pool)
        .await
        .unwrap();
    let store = SqliteStore::from_pool(pool.clone());
    let transaction = store.begin().await.unwrap();
    store.insert_node_info_tx(owner.clone()).await.unwrap();
    let topics = [p2panda::Topic::from(*board.as_bytes())].into();
    AddressBookStore::<_, NodeInfo>::set_topics_tx(&store, owner.node_id, topics)
        .await
        .unwrap();
    store.commit(transaction).await.unwrap();
    pool.close().await;
    start(settings).await
}

async fn publish_history(board: &BoardHandle, author: usize) -> Vec<(EventId, usize)> {
    let mut sent = Vec::new();
    for index in (author..EVENTS).step_by(3) {
        let id = board.publish(index.to_string().into_bytes()).await.unwrap();
        sent.push((id, index));
    }
    sent
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
    let node = start(&local_settings(&network, PathBuf::from(dir))).await;
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
