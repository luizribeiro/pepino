use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{BoardId, DeviceId, EventId, IdentityId};
use pepino_node::DeviceCredentials;
use serde::{Deserialize, Serialize};

use crate::{AppError, WireFormat};

use super::{
    edit::{Change, Edit, SignedEdit},
    fold::fold,
    record::Record,
};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct IdentityCatalog {
    pub(super) root: Option<DeviceId>,
    pub(super) edits: BTreeMap<EventId, SignedEdit>,
    invalid: BTreeSet<EventId>,
    rejected: BTreeSet<EventId>,
    pub(super) record: Option<Record>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) migration: Option<BTreeMap<DeviceId, String>>,
}

impl IdentityCatalog {
    pub(crate) fn edits(&self) -> impl Iterator<Item = &SignedEdit> {
        self.edits.values()
    }

    pub(super) fn rejected_count(&self) -> usize {
        self.invalid.len() + self.rejected.len()
    }

    pub(crate) fn merge(
        &mut self,
        board: BoardId,
        identity: IdentityId,
        incoming: impl IntoIterator<Item = SignedEdit>,
    ) -> Result<(), AppError> {
        let previous = self.edits.len();
        for signed in incoming {
            if let Some(id) = signed.id(board, identity)? {
                self.edits.entry(id).or_insert(signed);
            } else {
                let bytes = WireFormat::Cbor.encode(&signed)?;
                self.invalid
                    .insert(EventId::from_bytes(*blake3::hash(&bytes).as_bytes()));
            }
        }
        if previous != self.edits.len() {
            self.refresh();
        }
        Ok(())
    }

    pub(super) fn refresh(&mut self) {
        if let Some(root) = self.root {
            let folded = fold(&self.edits, root);
            self.record = Record::fold(
                folded
                    .ordered
                    .into_iter()
                    .map(|edit| (&edit.change, edit.writer)),
            );
            self.rejected = folded.rejected;
        }
    }

    pub(super) fn append(
        &mut self,
        board: BoardId,
        identity: IdentityId,
        keys: &DeviceCredentials,
        change: Change,
    ) -> Result<(), AppError> {
        let writer = keys.device_id();
        let parents = self
            .root
            .map(|root| fold(&self.edits, root).heads)
            .unwrap_or_default();
        let generation = parents
            .iter()
            .map(|id| self.edits[id].edit.generation.saturating_add(1))
            .max()
            .unwrap_or(0);
        let signed = SignedEdit::sign(
            Edit {
                writer,
                parents,
                generation,
                change,
            },
            board,
            identity,
            keys,
        )?;
        self.merge(board, identity, [signed])
    }
}

#[cfg(test)]
#[test]
fn signed_edits_are_deduplicated_after_delivery() {
    let dir = tempfile::tempdir().unwrap();
    let keys = DeviceCredentials::load_or_create(dir.path().join("titanium")).unwrap();
    let board = BoardId::from_bytes([48; 32]);
    let identity = IdentityId::from_bytes([49; 32]);
    let mut sender = IdentityCatalog {
        root: Some(keys.device_id()),
        ..Default::default()
    };
    let change = Change::Initialize {
        display_name: "Ada Gardener".into(),
        device_name: "Titanium".into(),
        added_ms: None,
    };
    sender.append(board, identity, &keys, change).unwrap();
    let edits = sender.edits.clone();
    sender.merge(board, identity, edits.into_values()).unwrap();
    assert_eq!(sender.edits.len(), 1);
    assert_eq!(sender.rejected_count(), 0);
    assert_eq!(sender.record.unwrap().display_name, "Ada Gardener");
}
