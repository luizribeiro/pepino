use pepino_app::{
    AppError, BoardAdmission, BoardCards, BoardRequest, BoardRole, CardExchange, IdentityCard,
};
use pepino_core::{BoardId, DeviceId};

use crate::{Error, SyncEngine, TransportBoard};

impl SyncEngine {
    pub async fn request_board(&self, name: Option<&str>) -> Result<BoardRequest, Error> {
        let member = self.node.member_info().await?;
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
        let local = self.app.fresh_local_state().await?;
        local.check_device_write()?;
        local.check_invitable(id)?;
        let board = local.resolve_board(&id.to_string(), "board")?;
        let denied = || Error::EnrollmentAccess {
            board: board.name.clone(),
        };
        workers.open_missing(&local).await?;
        let handle = workers.handle(id).ok_or_else(denied)?;
        if !handle.manages(local.device_id).await {
            return Err(denied());
        }
        if local.identity_id == request.identity.id
            || local.board_people(id).any(|person| {
                person.id == request.identity.id && person.first_device != request.device.id
            })
        {
            return Err(AppError::InvalidInput {
                field: "request".into(),
                reason: "This version only invites the first device of another identity.".into(),
                diagnostic: None,
            }
            .into());
        }
        if handle
            .members()
            .await?
            .iter()
            .any(|(device, existing)| *device == request.device.id && *existing != role.into())
        {
            return Err(AppError::InvalidInput {
                field: "role".into(),
                reason: "Changing a person's board role is reserved for a later release.".into(),
                diagnostic: None,
            }
            .into());
        }
        let (owner, owner_device) = owner_card(&local, id)?;
        let admission = BoardAdmission {
            owner: owner.clone(),
            owner_device,
            invitee: request.identity,
            device: request.device.id,
            at: std::time::SystemTime::now().into(),
        };
        handle.add_member(&member, role.into()).await?;
        self.app
            .record_board_admission(id, local.device_id, admission.clone())
            .await?;
        handle
            .publish(BoardCards::Introduce(admission.clone()).encode()?)
            .await?;
        let channel = if role == BoardRole::Read {
            let exchange = CardExchange {
                board: id,
                person: admission.invitee.id,
            };
            Some(self.create_card_exchange(&local, exchange, &member).await?)
        } else {
            None
        };
        self.nudge();
        Ok((owner, channel))
    }

    async fn create_card_exchange(
        &self,
        local: &pepino_app::LocalState,
        exchange: CardExchange,
        member: &pepino_node::MemberInfo,
    ) -> Result<BoardId, Error> {
        if let Some((&channel, _)) = local
            .card_exchanges()
            .iter()
            .find(|(_, saved)| **saved == exchange)
        {
            return Ok(channel);
        }
        let handle = self.node.create_board().await?;
        let channel = handle.id();
        let enroll = handle.add_member(member, pepino_node::Role::Write).await;
        let closed = handle.close().await;
        enroll?;
        closed?;
        self.app.register_card_exchange(channel, exchange).await?;
        Ok(channel)
    }
}

fn owner_card(
    local: &pepino_app::LocalState,
    id: BoardId,
) -> Result<(IdentityCard, DeviceId), AppError> {
    let owner = local
        .shared_access(id)
        .map_or(local.identity_id, |access| access.owner);
    if owner == local.identity_id {
        return Ok((local.identity_card()?, local.device_id));
    }
    let person = local
        .board_people(id)
        .find(|person| person.id == owner)
        .ok_or_else(|| AppError::InvalidInput {
            field: "board".into(),
            reason: "Wait for the board owner's identity card to sync.".into(),
            diagnostic: None,
        })?;
    let card = person.card();
    let active = person
        .devices()
        .find(|device| device.state == pepino_app::RosterState::Active)
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
