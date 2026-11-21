use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{BoardId, DeviceId, EventId, IdentityId};
use pepino_node::DeviceCredentials;
use serde::{Deserialize, Serialize};

use crate::{AppError, WireFormat};

use super::{
    edit::{Change, Edit, SignedEdit, portable_scope},
    fold::{Fold, fold, valid_names},
    record::Record,
};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct IdentityCatalog {
    #[serde(default)]
    projection_version: u8,
    pub(crate) root: Option<DeviceId>,
    pub(super) edits: BTreeMap<EventId, SignedEdit>,
    invalid: BTreeSet<EventId>,
    rejected: BTreeSet<EventId>,
    pub(super) record: Option<Record>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) migration: Option<BTreeMap<DeviceId, String>>,
}

impl IdentityCatalog {
    pub(super) fn with_root(root: DeviceId) -> Self {
        Self {
            root: Some(root),
            ..Self::default()
        }
    }

    pub(crate) fn refresh_projection(&mut self, board: BoardId, identity: IdentityId) {
        if self.projection_version < 4 && !self.edits.is_empty() {
            self.refresh(board, identity);
        }
        self.sanitize_names();
    }

    pub(crate) fn has_revoked_writer(
        &self,
        board: BoardId,
        identity: IdentityId,
        device: DeviceId,
    ) -> bool {
        self.root.is_some_and(|root| {
            self.fold(root, board, identity)
                .revoked_writers
                .contains(&device)
        })
    }

    fn sanitize_names(&mut self) {
        if let Some(record) = &mut self.record {
            record.sanitize_names();
        }
    }

    pub(crate) fn name(&self) -> Option<&str> {
        self.record
            .as_ref()
            .map(|record| record.display_name.as_str())
    }

    pub(crate) fn devices(&self) -> impl Iterator<Item = &crate::RosterDevice> {
        self.record
            .iter()
            .flat_map(|record| record.devices.values())
    }

    pub(crate) fn confirms(&self, id: DeviceId) -> bool {
        self.record
            .as_ref()
            .is_some_and(|record| record.confirmed.contains(&id) || self.root == Some(id))
    }

    pub(crate) fn confirms_revoked_device(&self, id: DeviceId) -> bool {
        self.device(id)
            .is_some_and(|row| row.state == crate::RosterState::Revoked)
            && (self.confirms(id)
                || self
                    .record
                    .as_ref()
                    .is_some_and(|record| record.retirement_confirmations.contains(&id)))
    }

    pub(crate) fn device(&self, id: DeviceId) -> Option<&super::record::RosterDevice> {
        self.record.as_ref()?.devices.get(&id)
    }

    pub(crate) fn edits(&self) -> impl Iterator<Item = &SignedEdit> {
        self.edits.values()
    }

    pub(crate) fn rejected_count(&self) -> usize {
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
            if let Some(id) = signed.id(portable_scope(), identity)? {
                if valid_names(&signed.edit.change, false) {
                    self.edits.entry(id).or_insert(signed);
                } else {
                    self.invalid.insert(id);
                }
            } else if let Some(id) = signed.id(board, identity)? {
                self.edits.entry(id).or_insert(signed);
            } else {
                let bytes = WireFormat::Cbor.encode(&signed)?;
                self.invalid
                    .insert(EventId::from_bytes(*blake3::hash(&bytes).as_bytes()));
            }
        }
        if previous != self.edits.len() {
            self.refresh(board, identity);
        }
        Ok(())
    }

    fn fold(&self, root: DeviceId, board: BoardId, identity: IdentityId) -> Fold<'_> {
        fold(&self.edits, root, |id, edit| {
            board != portable_scope()
                && edit
                    .digest(board, identity)
                    .is_ok_and(|digest| id.as_bytes() == &digest)
        })
    }

    pub(crate) fn portable_edits(&self, identity: IdentityId) -> Vec<SignedEdit> {
        self.root.map_or_else(Vec::new, |root| {
            let folded = self.fold(root, portable_scope(), identity);
            let portable_id = |edit: &Edit| {
                edit.digest(portable_scope(), identity)
                    .ok()
                    .map(EventId::from_bytes)
            };
            folded
                .ordered
                .into_iter()
                .filter_map(|edit| self.edits.get(&portable_id(edit)?))
                .chain(folded.evidence.iter().filter_map(|id| {
                    let signed = self.edits.get(id)?;
                    (portable_id(&signed.edit) == Some(*id)).then_some(signed)
                }))
                .cloned()
                .collect()
        })
    }

    pub(crate) fn trim_board_record(&mut self, identity: IdentityId) -> usize {
        let mut retained = Self {
            root: self.root,
            ..Self::default()
        };
        let accepted = self.portable_edits(identity);
        if retained
            .merge(portable_scope(), identity, accepted)
            .is_err()
        {
            return 0;
        }
        let dropped = self.edits.len().saturating_sub(retained.edits.len()) + self.invalid.len();
        *self = retained;
        dropped
    }

    pub(crate) fn fully_verified(&self, identity: IdentityId) -> bool {
        self.invalid.is_empty()
            && self.record.is_some()
            && self.root.is_some_and(|root| {
                let folded = self.fold(root, portable_scope(), identity);
                folded.ordered.len() + folded.evidence.len() == self.edits.len()
            })
    }

    pub(super) fn authored_by(
        &self,
        board: BoardId,
        identity: IdentityId,
        writer: DeviceId,
    ) -> bool {
        self.root.is_some_and(|root| {
            self.fold(root, board, identity)
                .ordered
                .iter()
                .any(|edit| edit.writer == writer)
        })
    }

    pub(super) fn refresh(&mut self, board: BoardId, identity: IdentityId) {
        if let Some(root) = self.root {
            let Fold {
                record, rejected, ..
            } = self.fold(root, board, identity);
            self.record = record;
            self.rejected = rejected;
            self.projection_version = 4;
            self.sanitize_names();
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
            .map(|root| self.fold(root, board, identity).heads)
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
            portable_scope(),
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

#[cfg(test)]
#[test]
fn new_roster_edits_can_be_verified_without_the_private_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let keys = DeviceCredentials::load_or_create(dir.path().join("titanium")).unwrap();
    let catalog = BoardId::from_bytes([48; 32]);
    let other_board = BoardId::from_bytes([51; 32]);
    let identity = IdentityId::from_bytes([49; 32]);
    let mut local = IdentityCatalog::with_root(keys.device_id());
    local
        .append(
            catalog,
            identity,
            &keys,
            Change::Initialize {
                display_name: "Ada Gardener".into(),
                device_name: "Titanium".into(),
                added_ms: None,
            },
        )
        .unwrap();
    let mut remote = IdentityCatalog::with_root(keys.device_id());
    remote
        .merge(other_board, identity, local.edits().cloned())
        .unwrap();
    assert_eq!(remote, local);
}

#[cfg(test)]
#[test]
fn timestamp_free_names_are_lax_only_in_private_legacy_edits() {
    let dir = tempfile::tempdir().unwrap();
    let keys = DeviceCredentials::load_or_create(dir.path().join("owner")).unwrap();
    let board = BoardId::from_bytes([48; 32]);
    let identity = IdentityId::from_bytes([49; 32]);
    let root = keys.device_id();
    let edit = Edit {
        writer: root,
        parents: Default::default(),
        generation: 0,
        change: Change::Initialize {
            display_name: "Ada\u{202e} Gardener".into(),
            device_name: "Garden\x1b tablet".into(),
            added_ms: None,
        },
    };
    let mut ledger = IdentityCatalog::with_root(root);
    let portable = SignedEdit::sign(edit.clone(), portable_scope(), identity, &keys).unwrap();
    ledger.merge(board, identity, [portable]).unwrap();
    assert_eq!(ledger.name(), None);
    assert_eq!(ledger.rejected_count(), 1);
    let legacy = SignedEdit::sign(edit, board, identity, &keys).unwrap();
    ledger.merge(board, identity, [legacy]).unwrap();
    assert_eq!(ledger.name(), Some("Ada Gardener"));
    assert_eq!(ledger.device(root).unwrap().name, "Garden tablet");

    let mut portable = IdentityCatalog::with_root(root);
    portable
        .append(
            board,
            identity,
            &keys,
            Change::Initialize {
                display_name: "Ada Gardener".into(),
                device_name: "Laptop".into(),
                added_ms: None,
            },
        )
        .unwrap();
    let device = DeviceId::from_bytes([79; 32]);
    portable
        .append(
            board,
            identity,
            &keys,
            Change::Add {
                device,
                name: "A".repeat(1025),
                added_ms: None,
            },
        )
        .unwrap();
    assert!(portable.device(device).is_none());
    assert_eq!(portable.rejected_count(), 1);
}
