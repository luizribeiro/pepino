#![allow(dead_code)]

use std::{fs, path::Path};

use pepino_app::{App, AppError, OpenOptions};
use pepino_core::{BoardId, IdentityId};
use pepino_node::DeviceCredentials;
use pepino_store::Store;
use serde_json::{Value, json};

pub fn prepare_blocking(directory: &Path) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(prepare(directory));
}

pub async fn prepare(directory: &Path) {
    prepare_board(directory, BoardId::from_bytes([73; 32])).await;
}

pub async fn open(directory: &Path, now: u64) -> Result<App, AppError> {
    prepare(directory).await;
    App::open(directory, now).await
}

pub async fn open_with_options(
    directory: &Path,
    now: u64,
    options: OpenOptions,
) -> Result<App, AppError> {
    prepare(directory).await;
    set_names(directory, &options);
    App::open_with_options(directory, now, options).await
}

pub fn set_names(directory: &Path, options: &OpenOptions) {
    let path = directory.join("local.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for (key, value) in [
        ("display_name", &options.display_name),
        ("device_name", &options.device_name),
    ] {
        if let Some(value) = value {
            state[key] = json!(value);
        }
    }
    if let Some(name) = &options.board_name {
        state["board_name"] = json!(name);
    }
    fs::write(path, serde_json::to_vec(&state).unwrap()).unwrap();
}

pub async fn prepare_board(directory: &Path, board: BoardId) {
    fs::create_dir_all(directory).unwrap();
    let credentials =
        DeviceCredentials::load_or_create(directory.join("credentials.cbor")).unwrap();
    let device = credentials.device_id();
    let mut identity = *device.as_bytes();
    identity[0] ^= 0x80;
    let identity = IdentityId::from_bytes(identity);
    let database = directory.join("pepino.sqlite");
    let store = Store::open(&database).await.unwrap();
    store.bind_local(board, identity, device).await.unwrap();
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
