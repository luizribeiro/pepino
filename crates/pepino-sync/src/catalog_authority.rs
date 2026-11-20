use std::path::Path;

use pepino_core::{BoardId, DeviceId};
use serde::Deserialize;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

#[derive(Deserialize)]
struct SpaceAuthority {
    group_id: p2panda_spaces::GroupId,
    groups_y: p2panda_spaces::AuthGroupState<()>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("read the catalog board's creation: {0}")]
    Sqlite(#[from] sqlx::Error),
    #[error("decode the catalog board's membership history: {0}")]
    Decode(String),
    #[error("the catalog board has multiple creation operations")]
    MultipleCreators,
}

pub(crate) async fn creator(path: &Path, board: BoardId) -> Result<Option<DeviceId>, Error> {
    let mut connection =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path).read_only(true))
            .await?;
    // A restarted node exposes current members through BoardHandle, while the
    // authenticated creation action remains in p2panda's processed space state.
    let bytes: Option<Vec<u8>> = sqlx::query_scalar("SELECT state FROM spaces_v1 WHERE id = ?")
        .bind(board.to_string())
        .fetch_optional(&mut connection)
        .await?;
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    let SpaceAuthority { group_id, groups_y } =
        p2panda_core::cbor::decode_cbor(bytes.as_slice())
            .map_err(|error| Error::Decode(error.to_string()))?;
    let mut creators = groups_y
        .inner
        .operations
        .values()
        .filter(|operation| operation.group_id == group_id && operation.action.is_create());
    let creator = creators
        .next()
        .map(|operation| DeviceId::from_bytes(*operation.author.as_bytes()));
    if creators.next().is_some() {
        return Err(Error::MultipleCreators);
    }
    Ok(creator)
}

#[cfg(test)]
#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn reads_the_creator_from_the_processed_membership_history() {
    use pepino_node::{DeviceCredentials, Node, NodeSettings};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("node.sqlite");
    let keys = DeviceCredentials::load_or_create(dir.path().join("titanium.cbor")).unwrap();
    let owner = keys.device_id();
    let node = Node::spawn(NodeSettings::offline(keys, &path))
        .await
        .unwrap();
    let catalog = node.create_board().await.unwrap();
    assert_eq!(creator(&path, catalog.id()).await.unwrap(), Some(owner));
    assert_eq!(
        creator(&path, BoardId::from_bytes([88; 32])).await.unwrap(),
        None
    );
    catalog.close().await.unwrap();
    node.shutdown().await.unwrap();
}
