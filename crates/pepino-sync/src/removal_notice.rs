use futures_util::StreamExt;
use pepino_app::App;
use pepino_core::DeviceId;
use pepino_node::{MemberState, MembershipChange, MembershipEvent, MembershipEvents};

pub(crate) async fn observe(app: &App, mut membership: MembershipEvents) {
    let own = app.local_state().device_id;
    let mut was_removed = false;
    while let Some(event) = membership.next().await {
        let Ok(event) = event else { continue };
        let readmitted =
            was_removed && matches!(event.members.get(&own), Some(MemberState::Member(_)));
        if let Some(by) = removal_actor(&event, own, &mut was_removed) {
            let _ = app
                .observe_catalog_removal(by, std::time::SystemTime::now().into())
                .await;
        } else if readmitted {
            let _ = app.clear_catalog_removal().await;
        }
    }
}

fn removal_actor(
    event: &MembershipEvent,
    own: DeviceId,
    was_removed: &mut bool,
) -> Option<DeviceId> {
    let removed = event.members.get(&own) == Some(&MemberState::Removed);
    let newly_removed = removed && !*was_removed;
    *was_removed = removed;
    match event.change {
        Some((by, MembershipChange::Removed)) if newly_removed => Some(by),
        _ => None,
    }
}

#[cfg(test)]
#[test]
fn only_a_transition_to_removed_attributes_a_board_action() {
    use pepino_node::Role;
    let laptop = DeviceId::from_bytes([56; 32]);
    let tablet = DeviceId::from_bytes([61; 32]);
    let mut removed = false;
    let mut event = MembershipEvent {
        members: [
            (laptop, MemberState::Member(Role::Manage)),
            (tablet, MemberState::Member(Role::Write)),
        ]
        .into(),
        change: Some((laptop, MembershipChange::Removed)),
    };
    assert_eq!(removal_actor(&event, tablet, &mut removed), None);
    event.members.insert(tablet, MemberState::Removed);
    assert_eq!(removal_actor(&event, tablet, &mut removed), Some(laptop));
    assert_eq!(removal_actor(&event, tablet, &mut removed), None);
}
