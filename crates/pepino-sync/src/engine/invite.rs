use pepino_app::{
    AppError, BoardAdmission, BoardCards, BoardRequest, BoardRole, CardExchange, IdentityCard,
};
use pepino_core::{BoardId, DeviceId};

use crate::{Error, SyncEngine};

impl SyncEngine {
    pub async fn request_board(&self, name: Option<&str>) -> Result<BoardRequest, Error> {
        let member = self.node.member_info().await?;
        self.app.remember_member_bundle(&member).await?;
        Ok(self.app.request_board(&member, name).await?)
    }

    pub async fn invite_board(
        &self,
        id: BoardId,
        request: BoardRequest,
        role: BoardRole,
    ) -> Result<(IdentityCard, Option<BoardId>), Error> {
        let member = request.member_info()?;
        request.name()?;
        let mut workers = self.boards.lock().await;
        let mut local = self.app.fresh_local_state().await?;
        local.check_device_write()?;
        local.check_invitable(id)?;
        local.check_person_id(request.identity.key())?;
        let board = local.resolve_board(&id.to_string(), "board")?;
        local.check_board_write(board)?;
        let denied = || Error::EnrollmentAccess {
            board: board.name.clone(),
        };
        workers.open_missing(&local).await?;
        let handle = workers.handle(id).ok_or_else(denied)?;
        let members = handle.members().await?;
        if !members.contains(&(local.device_id, pepino_node::Role::Manage)) {
            return Err(denied());
        }
        let person = request.identity.key();
        if local.membership_conflicts(id).contains(&request.device.id)
            || local
                .board_people(id)
                .any(|known| known.key() != person && known.confirms(request.device.id))
        {
            return Err(AppError::InvalidInput {
                field: "request".into(),
                reason: "The device has confirmed more than one identity on this board.".into(),
                diagnostic: None,
            }
            .into());
        }
        let former = local
            .latest_membership(id, person)
            .is_some_and(|record| record.change.action.role().is_none());
        if former {
            let previous = local.membership_devices(id, person);
            if members.iter().any(|(device, _)| previous.contains(device)) {
                return Err(AppError::InvalidInput {
                    field: "request".into(),
                    reason:
                        "Wait for the person's device removals to finish before inviting again."
                            .into(),
                    diagnostic: None,
                }
                .into());
            }
        }
        if local.own_person_key() == person
            || (!former
                && local
                    .board_people(id)
                    .any(|known| known.key() == person && known.first_device != request.device.id))
        {
            return Err(AppError::InvalidInput {
                field: "request".into(),
                reason: "This person is already a member of the board.".into(),
                diagnostic: None,
            }
            .into());
        }
        if members
            .iter()
            .any(|(device, existing)| *device == request.device.id && *existing != role.into())
        {
            return Err(AppError::InvalidInput {
                field: "role".into(),
                reason: "Use ppn board role to change an existing member's role.".into(),
                diagnostic: None,
            }
            .into());
        }
        let creator = match &workers.authority {
            Some((path, _)) => crate::catalog_authority::creator(path, id).await?,
            None => None,
        }
        .ok_or_else(|| AppError::InvalidInput {
            field: "board".into(),
            reason: "Wait for the board's creation record to sync.".into(),
            diagnostic: None,
        })?;
        local = self.app.confirm_roster_device().await?;
        let (owner, owner_device) = owner_card(&local, id)?;
        if !owner.authenticates_creator(creator, local.device_id)? {
            return Err(AppError::InvalidInput {
                field: "board".into(),
                reason: "The owner's roster needs a signed edit from this board's creator.".into(),
                diagnostic: None,
            }
            .into());
        }
        let admission = BoardAdmission {
            owner: owner.clone(),
            owner_device,
            invitee: request.identity.clone(),
            device: request.device.id,
            at: std::time::SystemTime::now().into(),
        };
        local.check_board_admission(id, local.device_id, creator, &admission)?;
        let introduce = BoardCards::Introduce(admission.clone()).encode()?;
        self.app.consume_board_request(id, &request).await?;
        let incarnation = if !former && let Some(saved) = local.membership_incarnation(id, person) {
            saved
        } else {
            crate::board_membership::publish_change(
                &self.app,
                &local,
                &*handle,
                id,
                person,
                pepino_app::MembershipAction::Invited(role),
            )
            .await?
        };
        local = self.app.fresh_local_state().await?;
        if local.membership_incarnation(id, person) != Some(incarnation) {
            return Err(Error::MembershipIncomplete(
                "a removal superseded this invitation".into(),
            ));
        }
        self.app
            .record_board_admission(id, local.device_id, creator, admission)
            .await?;
        let exchange = CardExchange {
            board: id,
            person: person.0,
            root: Some(person.1),
            incarnation: Some(incarnation),
        };
        let channel = self
            .create_card_exchange(&mut workers, &local, exchange, &member)
            .await?;
        handle.add_member(&member, role.into()).await?;
        handle.publish(introduce).await?;
        self.nudge();
        Ok((owner, Some(channel)))
    }

    pub(super) async fn create_card_exchange(
        &self,
        workers: &mut crate::board_workers::BoardWorkers<pepino_node::Node>,
        local: &pepino_app::LocalState,
        exchange: CardExchange,
        member: &pepino_node::MemberInfo,
    ) -> Result<BoardId, Error> {
        let recipient = member.device_id()?;
        if let Some(channel) = usable_card_exchange(workers, local, exchange, recipient).await {
            return Ok(channel);
        }
        let handle = self.node.create_board().await?;
        let channel = handle.id();
        let route = exchange
            .root
            .map(|root| {
                BoardCards::ReaderRoute {
                    channel,
                    person: (exchange.person, root),
                    incarnation: exchange.incarnation,
                }
                .encode()
            })
            .transpose()?;
        self.app.register_card_exchange(channel, exchange).await?;
        workers.start_card_exchange(handle, channel)?;
        if recipient != local.device_id {
            let handle = workers
                .handle(channel)
                .ok_or(Error::Node(pepino_node::Error::Closed))?;
            handle.add_member(member, pepino_node::Role::Write).await?;
        }
        if let (Some(board), Some(route)) = (workers.handle(exchange.board), route) {
            board.publish(route).await?;
        }
        Ok(channel)
    }
}

async fn usable_card_exchange(
    workers: &crate::board_workers::BoardWorkers<pepino_node::Node>,
    local: &pepino_app::LocalState,
    exchange: CardExchange,
    recipient: DeviceId,
) -> Option<BoardId> {
    for (&channel, saved) in local.card_exchanges() {
        if *saved == exchange
            && let Some(handle) = workers.handle(channel)
            && let Ok(members) = handle.members().await
            && members.contains(&(local.device_id, pepino_node::Role::Manage))
            && members.iter().any(|(device, _)| *device == recipient)
        {
            return Some(channel);
        }
    }
    None
}

fn owner_card(
    local: &pepino_app::LocalState,
    id: BoardId,
) -> Result<(IdentityCard, DeviceId), AppError> {
    let owner = local
        .shared_access(id)
        .map_or(local.identity_id, |access| access.owner);
    if local
        .shared_access(id)
        .is_none_or(|access| local.owns_share(access))
    {
        return Ok((local.identity_card()?, local.device_id));
    }
    let person = local
        .board_people(id)
        .find(|person| {
            person.id == owner
                && local
                    .shared_access(id)
                    .and_then(|access| access.owner_root)
                    .is_none_or(|root| person.key().1 == root)
                && person.invited_by.is_none()
        })
        .ok_or_else(|| AppError::InvalidInput {
            field: "board".into(),
            reason: "Wait for the board owner's identity card to sync.".into(),
            diagnostic: None,
        })?;
    let card = person.card();
    let active = person
        .devices()
        .find(|device| {
            person.confirms(device.id) && device.state == pepino_app::RosterState::Active
        })
        .ok_or_else(|| AppError::InvalidInput {
            field: "board".into(),
            reason: "The board owner's identity has no known active devices.".into(),
            diagnostic: None,
        })?;
    Ok((card, active.id))
}

#[cfg(test)]
#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn an_offline_engine_refuses_to_share_the_private_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let owner = pepino_app::App::init(dir.path().join("owner"), 0)
        .await
        .unwrap();
    let path = dir.path().join("owner");
    let keys = owner.identity_credentials().unwrap();
    let node = pepino_node::Node::spawn(pepino_node::NodeSettings::offline(
        keys,
        path.join("node.sqlite"),
    ))
    .await
    .unwrap();
    let engine = SyncEngine::start(Default::default(), std::sync::Arc::new(node), &path)
        .await
        .unwrap();
    let request = engine.request_board(Some("Garden tablet")).await.unwrap();
    let result = engine
        .invite_board(
            owner.local_state().catalog_board(),
            request,
            BoardRole::Write,
        )
        .await;
    assert!(
        matches!(result, Err(Error::App(AppError::InvalidInput { field, .. })) if field == "board")
    );
    engine.shutdown().await.unwrap();
}
