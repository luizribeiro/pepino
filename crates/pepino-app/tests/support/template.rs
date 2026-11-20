#![allow(dead_code)]

use std::{fs, path::Path, sync::OnceLock};

use pepino_app::{App, AppError, OpenOptions};
use pepino_core::{BoardId, IdentityId};
use pepino_node::DeviceCredentials;
use pepino_store::Store;
use serde_json::{Value, json};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use tempfile::TempDir;

const NOW: u64 = 1_795_144_800_000;

pub fn copy(directory: &Path) {
    copy_from(template(), directory);
}

pub async fn open(directory: &Path, now: u64) -> Result<App, AppError> {
    copy(directory);
    App::open(directory, now).await
}

pub async fn open_with_options(
    directory: &Path,
    now: u64,
    options: OpenOptions,
) -> Result<App, AppError> {
    copy(directory);
    set_names(directory, &options);
    App::open_with_options(directory, now, options).await
}

pub fn set_names(directory: &Path, options: &OpenOptions) {
    let path = directory.join("local.json");
    let mut state = state(directory);
    for (key, value) in [
        ("display_name", &options.display_name),
        ("device_name", &options.device_name),
        ("board_name", &options.board_name),
    ] {
        if let Some(value) = value {
            state[key] = json!(value);
        }
    }
    fs::write(path, serde_json::to_vec(&state).unwrap()).unwrap();
}

pub fn copy_peer(directory: &Path) {
    static PEER: OnceLock<TempDir> = OnceLock::new();
    let peer = PEER.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let board = state(template())["board_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        on_runtime(|runtime| runtime.block_on(store_fixture(dir.path(), board)));
        dir
    });
    copy_from(peer.path(), directory);
}

fn state(directory: &Path) -> Value {
    serde_json::from_slice(&fs::read(directory.join("local.json")).unwrap()).unwrap()
}

fn template() -> &'static Path {
    static TEMPLATE: OnceLock<TempDir> = OnceLock::new();
    TEMPLATE
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            on_runtime(|runtime| {
                runtime.block_on(async {
                    if cfg!(pepino_network_tests) {
                        drop(App::init(dir.path(), NOW).await.unwrap());
                        checkpoint(dir.path()).await;
                    } else {
                        store_fixture(dir.path(), BoardId::from_bytes([73; 32])).await;
                    }
                })
            });
            dir
        })
        .path()
}

fn on_runtime(work: impl FnOnce(tokio::runtime::Runtime) + Send) {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                work(
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap(),
                )
            })
            .join()
            .unwrap()
    })
}

async fn store_fixture(directory: &Path, board: BoardId) {
    let credentials =
        DeviceCredentials::load_or_create(directory.join("credentials.cbor")).unwrap();
    let device = credentials.device_id();
    let mut identity = *device.as_bytes();
    identity[0] ^= 0x80;
    let identity = IdentityId::from_bytes(identity);
    let database = directory.join("pepino.sqlite");
    let store = Store::open(&database).await.unwrap();
    store.bind_local(board, identity, device).await.unwrap();
    drop(store);
    checkpoint(directory).await;
    fs::write(
        directory.join("local.json"),
        serde_json::to_vec(&json!({
            "version": 3,
            "device_id": device,
            "identity_id": identity,
            "display_name": "Ada Gardener",
            "device_name": "Garden laptop",
            "board_id": board,
            "board_name": "Personal",
            "saved_views": {},
        }))
        .unwrap(),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            directory.join("local.json"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
}

async fn checkpoint(directory: &Path) {
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new().filename(directory.join("pepino.sqlite")),
    )
    .await
    .unwrap();
    let (busy, _, _): (u32, u32, u32) = sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(busy, 0);
    connection.close().await.unwrap();
}

fn copy_from(template: &Path, directory: &Path) {
    fs::create_dir_all(directory).unwrap();
    for name in [
        "local.json",
        "pepino.sqlite",
        "credentials.cbor",
        "node.sqlite",
    ] {
        let path = template.join(name);
        if path.exists() {
            fs::copy(path, directory.join(name)).unwrap();
        }
    }
}
