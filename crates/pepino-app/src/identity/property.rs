use pepino_core::{BoardId, DeviceId, IdentityId};
use pepino_node::DeviceCredentials;
use proptest::prelude::*;

use super::{IdentityCatalog, edit::Change};

#[test]
fn arrival_order_preserves_concurrent_adds_renames_and_rejections() {
    let dir = tempfile::tempdir().unwrap();
    let keys = |name| DeviceCredentials::load_or_create(dir.path().join(name)).unwrap();
    let titanium = keys("titanium");
    let ferrum = keys("ferrum");
    let stranger = keys("stranger");
    let board = BoardId::from_bytes([27; 32]);
    let identity = IdentityId::from_bytes([31; 32]);
    let root = titanium.device_id();
    let mut left = IdentityCatalog::with_root(root);
    let append = |replica: &mut IdentityCatalog, keys, change| {
        replica.append(board, identity, keys, change).unwrap();
    };
    append(
        &mut left,
        &titanium,
        Change::Initialize {
            display_name: "Ada Gardener".into(),
            device_name: "Titanium".into(),
            added_ms: None,
        },
    );
    let add = |device, name: &str| Change::Add {
        device,
        name: name.into(),
        added_ms: Some(1795144800000),
    };
    append(&mut left, &titanium, add(ferrum.device_id(), "Ferrum"));
    let mut right = left.clone();
    let cobalt = DeviceId::from_bytes([42; 32]);
    let copper = DeviceId::from_bytes([43; 32]);
    append(&mut left, &titanium, add(cobalt, "Cobalt"));
    append(&mut right, &ferrum, add(copper, "Copper"));
    append(
        &mut left,
        &titanium,
        Change::RenameDevice(cobalt, "Cobalt tablet".into()),
    );
    append(
        &mut right,
        &ferrum,
        Change::RenameDevice(copper, "Copper laptop".into()),
    );
    append(
        &mut left,
        &titanium,
        Change::RenameIdentity("Ada Workshop".into()),
    );
    append(
        &mut right,
        &ferrum,
        Change::RenameIdentity("Ada Greenhouse".into()),
    );
    append(
        &mut right,
        &stranger,
        Change::RenameIdentity("Intruder".into()),
    );
    left.merge(board, identity, right.edits.into_values())
        .unwrap();
    let at = chrono::DateTime::from_timestamp(1795145000, 0).unwrap();
    append(&mut left, &ferrum, Change::RequestLeave { at });
    assert_eq!(
        left.record.as_ref().unwrap().devices[&ferrum.device_id()].leave_requested_at,
        Some(at)
    );
    append(
        &mut left,
        &titanium,
        Change::Revoke {
            device: ferrum.device_id(),
            at,
        },
    );
    append(
        &mut left,
        &ferrum,
        Change::RenameIdentity("Revoked draft".into()),
    );
    let record = left.record.as_ref().unwrap();
    let revocation = record.devices[&ferrum.device_id()]
        .revocation
        .as_ref()
        .unwrap();
    assert_eq!(revocation.by, root);
    assert_eq!(revocation.at, at);
    assert!(
        record.devices[&ferrum.device_id()]
            .leave_requested_at
            .is_none()
    );
    assert_eq!(record.devices[&cobalt].added_by, Some(root));
    assert_eq!(record.devices[&copper].added_by, Some(ferrum.device_id()));
    assert_eq!(record.devices[&cobalt].name, "Cobalt tablet");
    assert_eq!(record.devices[&copper].name, "Copper laptop");
    assert!(matches!(
        record.display_name.as_str(),
        "Ada Workshop" | "Ada Greenhouse"
    ));
    assert_eq!(left.rejected_count(), 2);
    let expected = left.record;
    let edits = left.edits.into_values().collect::<Vec<_>>();
    proptest!(|(order in proptest::collection::vec(any::<u16>(), edits.len()))| {
        let mut arrivals = order.into_iter().zip(&edits).collect::<Vec<_>>();
        arrivals.sort_by_key(|(order, _)| *order);
        let mut replica = IdentityCatalog::with_root(root);
        for (_, edit) in arrivals {
            replica.merge(board, identity, [edit.clone()]).unwrap();
        }
        prop_assert_eq!(&replica.record, &expected);
        prop_assert_eq!(replica.rejected_count(), 2);
    });
}
