use std::collections::BTreeSet;

use pepino_core::{BoardId, DeviceId, EventId, IdentityId};
use pepino_node::DeviceCredentials;
use serde::{Deserialize, Serialize};

use crate::{AppError, WireFormat};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) enum Change {
    Initialize {
        display_name: String,
        device_name: String,
        added_ms: Option<u64>,
    },
    Add {
        device: DeviceId,
        name: String,
        added_ms: Option<u64>,
    },
    Revoke {
        device: DeviceId,
        at: chrono::DateTime<chrono::Utc>,
    },
    RenameIdentity(String),
    RenameDevice(DeviceId, String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct Edit {
    pub(super) writer: DeviceId,
    pub(super) parents: BTreeSet<EventId>,
    pub(super) generation: u64,
    pub(super) change: Change,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SignedEdit {
    pub(super) edit: Edit,
    #[serde(with = "serde_bytes")]
    pub(super) signature: [u8; 64],
}

impl SignedEdit {
    pub(super) fn sign(
        edit: Edit,
        board: BoardId,
        identity: IdentityId,
        keys: &DeviceCredentials,
    ) -> Result<Self, AppError> {
        let signature = keys.sign(&edit.digest(board, identity)?);
        Ok(Self { edit, signature })
    }

    pub(super) fn id(
        &self,
        board: BoardId,
        identity: IdentityId,
    ) -> Result<Option<EventId>, AppError> {
        let digest = self.edit.digest(board, identity)?;
        Ok(
            DeviceCredentials::verify(self.edit.writer, &digest, &self.signature)
                .then(|| EventId::from_bytes(digest)),
        )
    }
}

impl Edit {
    fn digest(&self, board: BoardId, identity: IdentityId) -> Result<[u8; 32], AppError> {
        let bytes = WireFormat::Cbor.encode(&(board, identity, self))?;
        Ok(blake3::derive_key(
            "Pepino identity catalog edit v1",
            &bytes,
        ))
    }
}

#[cfg(test)]
#[test]
fn edits_are_bound_to_the_writer_causal_parents_and_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let keys = DeviceCredentials::load_or_create(dir.path().join("titanium.cbor")).unwrap();
    let board = BoardId::from_bytes([12; 32]);
    let identity = IdentityId::from_bytes([13; 32]);
    let parent = EventId::from_bytes([14; 32]);
    let edit = Edit {
        writer: keys.device_id(),
        parents: [parent].into(),
        generation: 3,
        change: Change::RenameIdentity("Ada Gardener".into()),
    };
    let mut signed = SignedEdit::sign(edit, board, identity, &keys).unwrap();
    assert!(signed.id(board, identity).unwrap().is_some());
    assert!(
        signed
            .id(board, IdentityId::from_bytes([15; 32]))
            .unwrap()
            .is_none()
    );
    assert!(
        signed
            .id(BoardId::from_bytes([16; 32]), identity)
            .unwrap()
            .is_none()
    );
    signed.edit.parents.remove(&parent);
    assert!(signed.id(board, identity).unwrap().is_none());
}
