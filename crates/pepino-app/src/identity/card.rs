use std::collections::BTreeMap;

use pepino_core::{DeviceId, IdentityId};
use pepino_node::MemberInfo;
use serde::{Deserialize, Serialize};

use super::{IdentityCatalog, SignedEdit, edit::portable_scope};
use crate::{AppError, LocalState, RosterState, app::invalid};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityCard {
    pub id: IdentityId,
    pub root: DeviceId,
    edits: Vec<SignedEdit>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) bundles: BTreeMap<DeviceId, String>,
}

impl IdentityCard {
    pub fn key(&self) -> crate::PersonKey {
        (self.id, self.root)
    }

    pub(crate) fn same_roster(&self, other: &Self) -> bool {
        self.key() == other.key() && self.edits == other.edits
    }

    pub(crate) fn record(&self) -> Result<IdentityCatalog, AppError> {
        let mut record = IdentityCatalog::with_root(self.root);
        self.merge_into(&mut record)?;
        Ok(record)
    }

    pub(crate) fn verified_record(&self) -> Result<IdentityCatalog, AppError> {
        let record = self.record()?;
        if !record.fully_verified(self.id) {
            return Err(invalid(
                "identity",
                "the identity card has unverified roster edits",
            ));
        }
        Ok(record)
    }

    pub(crate) fn merge_into(&self, record: &mut IdentityCatalog) -> Result<(), AppError> {
        record.merge(portable_scope(), self.id, self.edits.iter().cloned())
    }

    pub(crate) fn from_ledger(
        id: IdentityId,
        record: &IdentityCatalog,
        fallback: DeviceId,
    ) -> Self {
        Self {
            id,
            root: record.root.unwrap_or(fallback),
            edits: record.portable_edits(id),
            bundles: BTreeMap::new(),
        }
    }

    pub(crate) fn merge_bundles(
        &self,
        record: &IdentityCatalog,
        bundles: &mut BTreeMap<DeviceId, String>,
    ) {
        bundles.retain(|id, _| {
            record
                .device(*id)
                .is_some_and(|row| row.state == RosterState::Active)
        });
        for (&id, encoded) in &self.bundles {
            if !record
                .device(id)
                .is_some_and(|row| row.state == RosterState::Active)
            {
                continue;
            }
            if let Ok(info) = MemberInfo::from_hex(encoded)
                && info.device_id().is_ok_and(|device| device == id)
            {
                let previous = bundles
                    .get(&id)
                    .and_then(|saved| MemberInfo::from_hex(saved).ok());
                let order = |info: &MemberInfo| (info.expires_at().ok(), info.to_hex());
                if previous
                    .as_ref()
                    .is_none_or(|saved| order(&info) > order(saved))
                {
                    bundles.insert(id, encoded.clone());
                }
            }
        }
    }

    pub fn authenticates_creator(
        &self,
        creator: DeviceId,
        signer: DeviceId,
    ) -> Result<bool, AppError> {
        let record = self.record()?;
        Ok(record.device(creator).is_some()
            && (signer == creator || record.authored_by(portable_scope(), self.id, creator)))
    }

    pub fn name_for(&self, member: DeviceId) -> Result<String, AppError> {
        let catalog = self.verified_record()?;
        match catalog.device(member) {
            Some(row) if row.state == RosterState::Active && catalog.confirms(member) => {
                Ok(catalog.name().unwrap_or_default().to_owned())
            }
            _ => Err(invalid(
                "identity",
                "this device has not confirmed active membership of the identity",
            )),
        }
    }
}

impl LocalState {
    pub fn roster_device_confirmed(&self, device: DeviceId) -> bool {
        self.catalog.identity.confirms(device)
    }

    pub fn needs_roster_confirmation(&self) -> bool {
        self.active_roster_device(self.device_id)
            && !self.roster_device_confirmed(self.device_id)
            && self.roster_root().is_some()
            && !self.catalog.identity.has_revoked_writer(
                self.catalog_board(),
                self.identity_id,
                self.device_id,
            )
    }

    pub fn identity_card(&self) -> Result<IdentityCard, AppError> {
        let root = self
            .roster_root()
            .ok_or_else(|| invalid("identity", "wait for the identity roster to sync"))?;
        let card = IdentityCard {
            id: self.identity_id,
            root,
            edits: self.catalog.identity.portable_edits(self.identity_id),
            bundles: self
                .catalog
                .identity
                .devices()
                .filter_map(|row| {
                    if row.state != RosterState::Active {
                        return None;
                    }
                    self.device_bundle(row.id)
                        .map(|info| (row.id, info.to_hex()))
                })
                .collect(),
        };
        if card.record()?.name() != Some(self.display_name()) {
            return Err(invalid(
                "identity",
                "this roster needs a portable identity record before sharing",
            ));
        }
        card.name_for(self.device_id)?;
        Ok(card)
    }
}

impl crate::App {
    pub async fn confirm_roster_device(&self) -> Result<LocalState, AppError> {
        let keys = self.identity_credentials()?;
        self.try_update_local_state(move |local| {
            local.check_device_write()?;
            if !local.active_roster_device(keys.device_id()) {
                return Err(invalid(
                    "identity",
                    "wait for this device's identity roster to sync",
                ));
            }
            if local.needs_roster_confirmation()
                && let Some(root) = local.roster_root()
            {
                local.append_identity(&keys, super::edit::Change::ConfirmOwnership { root })?;
            }
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
#[tokio::test]
async fn a_card_binds_the_identity_name_and_active_devices() {
    let directory = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(directory.path(), 0)
        .await
        .unwrap();
    let keys = app.identity_credentials().unwrap();
    let local = app
        .reconcile_identity(keys.device_id(), &keys)
        .await
        .unwrap();
    let mut card = local.identity_card().unwrap();
    assert_eq!(
        card.name_for(local.device_id).unwrap(),
        local.display_name()
    );
    assert!(card.name_for(DeviceId::from_bytes([66; 32])).is_err());
    card.id = IdentityId::from_bytes([67; 32]);
    assert!(card.name_for(local.device_id).is_err());
}

#[cfg(test)]
#[test]
fn card_updates_converge_without_authorizing_an_edit_from_before_admission() {
    use super::edit::{Change, Edit};
    use proptest::prelude::*;

    let dir = tempfile::tempdir().unwrap();
    let owner = pepino_node::DeviceCredentials::load_or_create(dir.path().join("owner")).unwrap();
    let guest = pepino_node::DeviceCredentials::load_or_create(dir.path().join("guest")).unwrap();
    let id = IdentityId::from_bytes([85; 32]);
    let scope = portable_scope();
    let root = owner.device_id();
    let sign = |keys: &pepino_node::DeviceCredentials, parent, generation, change| {
        SignedEdit::sign(
            Edit {
                writer: keys.device_id(),
                parents: parent,
                generation,
                change,
            },
            scope,
            id,
            keys,
        )
        .unwrap()
    };
    let initialize = sign(
        &owner,
        Default::default(),
        0,
        Change::Initialize {
            display_name: "Ada Gardener".into(),
            device_name: "Laptop".into(),
            added_ms: None,
        },
    );
    let parent = initialize.id(scope, id).unwrap().unwrap();
    let forged = sign(
        &guest,
        [parent].into(),
        1,
        Change::RenameIdentity("Untrusted label".into()),
    );
    let admit = sign(
        &owner,
        [parent].into(),
        1,
        Change::Add {
            device: guest.device_id(),
            name: "Garden tablet".into(),
            added_ms: None,
        },
    );
    let admitted = admit.id(scope, id).unwrap().unwrap();
    let rename = sign(
        &guest,
        [admitted].into(),
        2,
        Change::RenameIdentity("Renamed gardener".into()),
    );
    let edits = [initialize, forged, admit, rename];
    proptest!(|(priority in any::<[u8; 4]>())| {
        let mut indices = [0, 1, 2, 3];
        indices.sort_by_key(|&index| priority[index]);
        let mut record = IdentityCatalog::with_root(root);
        for index in indices {
            IdentityCard { id, root, edits: vec![edits[index].clone()], bundles: Default::default() }.merge_into(&mut record).unwrap();
        }
        prop_assert_eq!(record.name(), Some("Renamed gardener"));
        prop_assert_eq!(record.rejected_count(), 1);
    });
}

#[cfg(test)]
#[tokio::test]
async fn a_bundle_and_roster_claim_do_not_establish_device_ownership() {
    use super::edit::Change;
    use crate::{BoardAdmission, BoardPerson};
    use pepino_core::BoardId;

    let dir = tempfile::tempdir().unwrap();
    let owner = crate::App::init(dir.path().join("owner"), 0).await.unwrap();
    let guest = crate::App::init(dir.path().join("guest"), 0).await.unwrap();
    let owner_keys = owner.identity_credentials().unwrap();
    let guest_keys = guest.identity_credentials().unwrap();
    let board = BoardId::from_bytes([92; 32]);
    owner
        .register_created_board(board, "Greenhouse")
        .await
        .unwrap();
    let mut local = owner.fresh_local_state().await.unwrap();
    let mut claimant = guest.fresh_local_state().await.unwrap();
    let node = pepino_node::Node::spawn(pepino_node::NodeSettings::offline(
        owner_keys.clone(),
        dir.path().join("bundle.sqlite"),
    ))
    .await
    .unwrap();
    let bundle = node.member_info().await.unwrap();
    let device = node.device_id();
    claimant
        .append_identity(
            &guest_keys,
            Change::Add {
                device,
                name: "Greenhouse laptop".into(),
                added_ms: None,
            },
        )
        .unwrap();
    let mut claim = claimant.identity_card().unwrap();
    claim.bundles.insert(device, bundle.to_hex());
    owner
        .record_board_admission(
            board,
            local.device_id,
            local.device_id,
            BoardAdmission {
                owner: local.identity_card().unwrap(),
                owner_device: local.device_id,
                invitee: claim.clone(),
                device: claimant.device_id,
                at: chrono::DateTime::from_timestamp(1795145000, 0).unwrap(),
            },
        )
        .await
        .unwrap();
    local = owner.fresh_local_state().await.unwrap();
    let owner_devices = local.membership_devices(board, local.own_person_key());
    let claimant_devices = local.membership_devices(board, claimant.own_person_key());
    assert!(owner_devices.contains(&device));
    assert!(!claimant_devices.contains(&device));
    assert!(local.membership_conflicts(board).is_empty());
    let claimed = BoardPerson::introduced(&claim, claimant.device_id, None).unwrap();
    assert!(
        crate::reconcile_person(
            &claimed,
            &claimant_devices,
            &Default::default(),
            Some(pepino_node::Role::Write)
        )
        .is_empty()
    );

    claimant
        .append_identity(&owner_keys, Change::ConfirmOwnership { root: claim.root })
        .unwrap();
    owner
        .receive_board_cards(
            board,
            claimant.device_id,
            None,
            crate::BoardCards::Update(claimant.identity_card().unwrap()),
        )
        .await
        .unwrap();
    let local = owner.fresh_local_state().await.unwrap();
    assert_eq!(local.membership_conflicts(board), [device].into());
    assert!(
        local
            .membership_devices(board, claimant.own_person_key())
            .contains(&device)
    );
    assert!(
        local
            .membership_devices(board, local.own_person_key())
            .contains(&device)
    );
    node.shutdown().await.unwrap();
}

#[cfg(test)]
#[tokio::test]
async fn credentials_without_a_confirmation_cannot_be_used_to_enroll_a_device() {
    use super::edit::Change;

    let dir = tempfile::tempdir().unwrap();
    let owner = crate::App::init(dir.path().join("owner"), 0).await.unwrap();
    let guest = crate::App::init(dir.path().join("guest"), 0).await.unwrap();
    let tablet_keys =
        pepino_node::DeviceCredentials::load_or_create(dir.path().join("tablet")).unwrap();
    let tablet = pepino_node::Node::spawn(pepino_node::NodeSettings::offline(
        tablet_keys.clone(),
        dir.path().join("tablet.sqlite"),
    ))
    .await
    .unwrap();
    let bundle = tablet.member_info().await.unwrap();
    let mut person = guest.fresh_local_state().await.unwrap();
    let guest_keys = guest.identity_credentials().unwrap();
    let device = tablet.device_id();
    person
        .append_identity(
            &guest_keys,
            Change::Add {
                device,
                name: "Greenhouse tablet".into(),
                added_ms: None,
            },
        )
        .unwrap();
    let mut claim = person.identity_card().unwrap();
    claim.bundles.insert(device, bundle.to_hex());
    let root = claim.root;
    let board = pepino_core::BoardId::from_bytes([71; 32]);
    owner
        .register_created_board(board, "Greenhouse")
        .await
        .unwrap();
    let local = owner.fresh_local_state().await.unwrap();
    owner
        .record_board_admission(
            board,
            local.device_id,
            local.device_id,
            crate::BoardAdmission {
                owner: local.identity_card().unwrap(),
                owner_device: local.device_id,
                invitee: claim,
                device: person.device_id,
                at: std::time::UNIX_EPOCH.into(),
            },
        )
        .await
        .unwrap();
    let local = owner.fresh_local_state().await.unwrap();
    let saved = local
        .board_people(board)
        .find(|saved| saved.id == person.identity_id)
        .unwrap();
    let transport = [(person.device_id, pepino_node::Role::Write)].into();
    let plan = crate::reconcile_person(
        saved,
        &local.membership_devices(board, saved.key()),
        &transport,
        Some(pepino_node::Role::Write),
    );
    assert!(plan.is_empty());

    person
        .append_identity(&tablet_keys, Change::ConfirmOwnership { root })
        .unwrap();
    owner
        .receive_board_cards(
            board,
            person.device_id,
            None,
            crate::BoardCards::Update(person.identity_card().unwrap()),
        )
        .await
        .unwrap();
    let local = owner.fresh_local_state().await.unwrap();
    let saved = local
        .board_people(board)
        .find(|saved| saved.id == person.identity_id)
        .unwrap();
    let plan = crate::reconcile_person(
        saved,
        &local.membership_devices(board, saved.key()),
        &transport,
        Some(pepino_node::Role::Write),
    );
    assert_eq!(
        plan,
        [crate::DeviceMembershipChange::Enroll(
            Box::new(bundle),
            pepino_node::Role::Write
        )]
    );
    tablet.shutdown().await.unwrap();
}

#[cfg(test)]
#[tokio::test]
async fn competing_manager_plans_repair_reordered_enrollments_and_removals() {
    use super::edit::Change;
    use crate::{BoardPerson, DeviceMembershipChange, reconcile_person};
    use pepino_node::Role;
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    let directory = tempfile::tempdir().unwrap();
    let app = crate::App::init(directory.path(), 0).await.unwrap();
    let keys = pepino_node::DeviceCredentials::load_or_create(directory.path().join("tablet.cbor"))
        .unwrap();
    let tablet = pepino_node::Node::spawn(pepino_node::NodeSettings::offline(
        keys.clone(),
        directory.path().join("tablet.sqlite"),
    ))
    .await
    .unwrap();
    let bundle = tablet.member_info().await.unwrap();
    let laptop = app.local_state().device_id;
    let phone_keys =
        pepino_node::DeviceCredentials::load_or_create(directory.path().join("phone.cbor"))
            .unwrap();
    let phone = phone_keys.device_id();
    app.remember_device(phone, "Old phone".into(), None)
        .await
        .unwrap();
    app.remember_device(tablet.device_id(), "Garden tablet".into(), None)
        .await
        .unwrap();
    app.remember_member_bundle(&bundle).await.unwrap();
    let active = app
        .try_update_local_state(move |local| {
            let root = local.roster_root().unwrap();
            for keys in [&phone_keys, &keys] {
                local.append_identity(keys, Change::ConfirmOwnership { root })?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let at = chrono::DateTime::from_timestamp(1795145000, 0).unwrap();
    let final_state = app.record_device_revocation(phone, at).await.unwrap();
    let person = |local: &LocalState| {
        BoardPerson::introduced(&local.identity_card().unwrap(), laptop, None).unwrap()
    };
    let old = person(&active);
    let desired = person(&final_state);
    let devices = [laptop, tablet.device_id(), phone].into();
    let apply = |members: &mut BTreeMap<_, _>, change: &DeviceMembershipChange| match change {
        DeviceMembershipChange::Enroll(info, role) => {
            members.insert(info.device_id().unwrap(), *role);
        }
        DeviceMembershipChange::Remove(id) => {
            members.remove(id);
        }
        DeviceMembershipChange::SetRole(id, role) => {
            if let Some(existing) = members.get_mut(id) {
                *existing = *role;
            }
        }
    };
    let mut rng = proptest::test_runner::TestRunner::default();
    rng.run(
        &proptest::collection::vec((any::<bool>(), any::<bool>(), any::<u8>()), 1..30),
        |actions| {
            let mut observed = [(laptop, Role::Write), (phone, Role::Write)].into();
            let mut delayed = Vec::new();
            for (latest, apply_now, priority) in actions {
                let (view, role) = if latest {
                    (&desired, Role::Read)
                } else {
                    (&old, Role::Write)
                };
                let plan = reconcile_person(view, &devices, &observed, Some(role));
                if apply_now {
                    for change in &plan {
                        apply(&mut observed, change);
                    }
                } else {
                    delayed.extend(plan.into_iter().map(|change| (priority, change)));
                }
            }
            delayed.sort_by_key(|(priority, _)| *priority);
            for (_, change) in delayed {
                apply(&mut observed, &change);
            }
            for change in reconcile_person(&desired, &devices, &observed, Some(Role::Read)) {
                apply(&mut observed, &change);
            }
            prop_assert_eq!(
                &observed,
                &[(laptop, Role::Read), (tablet.device_id(), Role::Read)].into()
            );
            for change in reconcile_person(&desired, &devices, &observed, None) {
                apply(&mut observed, &change);
            }
            prop_assert!(observed.is_empty());
            Ok(())
        },
    )
    .unwrap();
    tablet.shutdown().await.unwrap();
}

#[cfg(test)]
#[test]
fn private_and_board_rosters_agree_on_concurrent_device_retirement() {
    use super::edit::Change;
    use crate::{BoardPerson, DeviceMembershipChange, reconcile_person};
    use pepino_node::{DeviceCredentials, Role};

    let dir = tempfile::tempdir().unwrap();
    let root = DeviceCredentials::load_or_create(dir.path().join("laptop")).unwrap();
    let tablet = DeviceCredentials::load_or_create(dir.path().join("tablet")).unwrap();
    let phone = DeviceCredentials::load_or_create(dir.path().join("phone")).unwrap();
    let anchor = root.device_id();
    let identity = IdentityId::from_bytes([60; 32]);
    let origin = portable_scope();
    let mut base = IdentityCatalog::with_root(anchor);
    base.append(
        origin,
        identity,
        &root,
        Change::Initialize {
            display_name: "Ada Gardener".into(),
            device_name: "Laptop".into(),
            added_ms: None,
        },
    )
    .unwrap();
    let mut before_phone_confirmation = None;
    for (keys, name) in [(&tablet, "Greenhouse tablet"), (&phone, "Garden phone")] {
        base.append(
            origin,
            identity,
            &root,
            Change::Add {
                device: keys.device_id(),
                name: name.into(),
                added_ms: None,
            },
        )
        .unwrap();
        if keys.device_id() == phone.device_id() {
            before_phone_confirmation = Some(base.clone());
        }
        base.append(
            origin,
            identity,
            keys,
            Change::ConfirmOwnership { root: anchor },
        )
        .unwrap();
    }

    let before_phone_confirmation = before_phone_confirmation.unwrap();
    for (first, second, first_roster, confirmation_is_concurrent) in [
        (&tablet, &phone, &base, false),
        (&root, &tablet, &base, false),
        (&tablet, &root, &base, false),
        (&tablet, &phone, &before_phone_confirmation, true),
        (&root, &phone, &before_phone_confirmation, true),
    ] {
        let mut first_view = first_roster.clone();
        let mut second_view = base.clone();
        let at = chrono::DateTime::from_timestamp(1795145000, 0).unwrap();
        let spare = DeviceId::from_bytes([88; 32]);
        if first.device_id() == anchor {
            second_view
                .append(
                    origin,
                    identity,
                    second,
                    Change::Add {
                        device: spare,
                        name: "Workshop spare".into(),
                        added_ms: None,
                    },
                )
                .unwrap();
        }
        first_view
            .append(
                origin,
                identity,
                first,
                Change::RenameDevice(first.device_id(), "Work device".into()),
            )
            .unwrap();
        first_view
            .append(
                origin,
                identity,
                first,
                Change::Revoke {
                    device: second.device_id(),
                    at,
                },
            )
            .unwrap();
        second_view
            .append(
                origin,
                identity,
                second,
                Change::Revoke {
                    device: first.device_id(),
                    at,
                },
            )
            .unwrap();
        let card = |record| IdentityCard::from_ledger(identity, record, anchor);

        let mut private = first_view.clone();
        private
            .merge(origin, identity, second_view.edits().cloned())
            .unwrap();
        let mut reversed = second_view.clone();
        reversed
            .merge(origin, identity, first_view.edits().cloned())
            .unwrap();
        assert_eq!(private, reversed);

        let mut board = BoardPerson::introduced(&card(&base), anchor, None).unwrap();
        assert!(board.merge(&card(&first_view)).unwrap());
        assert!(board.merge(&card(&second_view)).unwrap());
        assert!(private.device(spare).is_none());
        assert!(!board.confirms(spare));
        assert!(board.devices().eq(private.devices()));
        assert!(
            board
                .card()
                .verified_record()
                .unwrap()
                .devices()
                .eq(private.devices())
        );
        let devices = board
            .devices()
            .filter(|row| board.confirms(row.id))
            .map(|row| row.id)
            .chain(board.confirmed_revoked_devices())
            .collect();
        if confirmation_is_concurrent && first.device_id() == anchor {
            assert!(!board.confirms(phone.device_id()));
            assert!(board.confirms_revoked_device(phone.device_id()));
        }
        let enrolled = [
            (anchor, Role::Write),
            (tablet.device_id(), Role::Write),
            (phone.device_id(), Role::Write),
        ]
        .into();
        let revoked = if first.device_id() == anchor {
            Some(second.device_id())
        } else if second.device_id() == anchor {
            Some(first.device_id())
        } else {
            None
        };
        assert_eq!(
            reconcile_person(&board, &devices, &enrolled, Some(Role::Write)),
            revoked
                .into_iter()
                .map(DeviceMembershipChange::Remove)
                .collect::<Vec<_>>(),
        );

        for keys in [&root, &tablet, &phone] {
            let row = private.device(keys.device_id()).unwrap();
            let expected = if Some(keys.device_id()) == revoked {
                RosterState::Revoked
            } else {
                RosterState::Active
            };
            assert_eq!(row.state, expected);
        }
    }
}
