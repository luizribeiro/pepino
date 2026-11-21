use pepino_core::BoardId;
use serde::{Deserialize, Serialize};

use crate::{
    App, AppError, BoardGrant, BoardJoinState, LocalState, PeerHint, SharedAccess, SignedGrant,
    app::invalid,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct JoinedShare {
    peer_id: pepino_core::DeviceId,
    relay_url: Option<String>,
    pub(crate) binding: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    receipt: Option<String>,
}

impl LocalState {
    pub fn shared_peers(&self) -> impl Iterator<Item = PeerHint> + '_ {
        self.catalog.joined_shares.values().map(|share| PeerHint {
            id: share.peer_id,
            relay_url: share.relay_url.clone(),
        })
    }

    fn install_share(&mut self, grant: &BoardGrant, receipt: String) -> Result<(), AppError> {
        let id = grant.board.id;
        self.check_invitable(id)?;
        if self
            .catalog
            .board_request
            .as_ref()
            .is_none_or(|request| request.identity.id != self.identity_id)
            || grant.recipient != self.device_id
            || grant.owner.id == self.identity_id
        {
            return Err(invalid(
                "grant",
                "the board grant must join a different identity",
            ));
        }
        if let Some((board, _)) = self
            .catalog
            .joined_shares
            .iter()
            .find(|(_, share)| share.binding == grant.request)
        {
            return if *board == id {
                Ok(())
            } else {
                Err(invalid("grant", "this board request has already been used"))
            };
        }
        if self.boards().iter().any(|board| board.id == id) {
            return Err(invalid("grant", "this board is already in your catalog"));
        }
        self.catalog.shared.insert(
            id,
            SharedAccess {
                owner: grant.owner.id,
                role: grant.role,
                admitted: self.device_id,
            },
        );
        self.name_board(id, &grant.board.name);
        if let Some(channel) = grant.card_exchange {
            self.add_card_exchange(
                channel,
                crate::CardExchange {
                    board: id,
                    person: self.identity_id,
                },
            )?;
        }
        self.catalog.joined_shares.insert(
            id,
            JoinedShare {
                peer_id: grant.peer.id,
                relay_url: grant.peer.relay_url.clone(),
                binding: grant.request.clone(),
                receipt: Some(receipt),
            },
        );
        self.catalog.board_request = None;
        Ok(())
    }
}

impl App {
    pub async fn join_board(
        &self,
        signed: SignedGrant,
    ) -> Result<(BoardId, PeerHint, BoardJoinState), AppError> {
        self.identity_credentials()?;
        let before = self.fresh_local_state().await?;
        before.check_device_write()?;
        let receipt = signed.receipt();
        if let Some((&board, share)) = before
            .catalog
            .joined_shares
            .iter()
            .find(|(_, share)| share.receipt.as_ref() == Some(&receipt))
        {
            return Ok((
                board,
                PeerHint {
                    id: share.peer_id,
                    relay_url: share.relay_url.clone(),
                },
                before.board_join_state(board),
            ));
        }
        let request = before
            .catalog
            .board_request
            .as_ref()
            .ok_or_else(|| invalid("grant", "Run ppn board request first"))?;
        request.member_info()?;
        let grant = BoardGrant::verify(signed, request)?;
        let board = grant.board.id;
        let peer = grant.peer.clone();
        let binding = grant.request.clone();
        let local = self
            .try_update_local_state(move |local| {
                local.check_device_write()?;
                if local
                    .catalog
                    .joined_shares
                    .get(&board)
                    .is_some_and(|saved| saved.receipt.as_ref() == Some(&receipt))
                {
                    return Ok(());
                }
                if local
                    .catalog
                    .board_request
                    .as_ref()
                    .map(crate::BoardRequest::binding)
                    .transpose()?
                    .as_ref()
                    != Some(&binding)
                {
                    return Err(invalid(
                        "grant",
                        "this grant does not match the saved request",
                    ));
                }
                local.install_share(&grant, receipt)
            })
            .await?;
        Ok((board, peer, local.board_join_state(board)))
    }
}

#[cfg(test)]
#[tokio::test]
async fn joining_adds_one_board_to_the_existing_local_catalog() {
    use crate::{BoardRequest, BoardRole, DeviceRequest, LocalBoard};
    use pepino_core::IdentityId;

    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    let keys = app.identity_credentials().unwrap();
    let mut local = app
        .reconcile_identity(keys.device_id(), &keys)
        .await
        .unwrap();
    let card = local.identity_card().unwrap();
    let private = local.catalog_board();
    local.catalog.board_request = Some(BoardRequest {
        device: DeviceRequest {
            id: local.device_id,
            name: "Tablet".into(),
            member_info: "fixture".into(),
            secret: Some([81; 32]),
        },
        identity: card.clone(),
        issuance: None,
    });
    let mut owner = card;
    owner.id = IdentityId::from_bytes([82; 32]);
    let id = BoardId::from_bytes([83; 32]);
    let mut grant = BoardGrant {
        recipient: local.device_id,
        request: "saved binding".into(),
        board: LocalBoard {
            id,
            name: "House".into(),
        },
        role: BoardRole::Read,
        card_exchange: Some(BoardId::from_bytes([86; 32])),
        owner,
        peer: PeerHint {
            id: pepino_core::DeviceId::from_bytes([84; 32]),
            relay_url: None,
        },
    };
    local.install_share(&grant, "grant digest".into()).unwrap();
    assert!(local.catalog.board_request.is_none());
    assert_eq!(local.boards().len(), 2);
    assert_eq!(local.catalog_board(), private);
    assert_eq!(local.card_exchanges().values().next().unwrap().board, id);
    assert_eq!(local.board_join_state(id), BoardJoinState::WaitingToBeAdded);
    assert!(local.shared_peers().next() == Some(grant.peer.clone()));
    grant.board.id = BoardId::from_bytes([85; 32]);
    assert!(local.install_share(&grant, "other digest".into()).is_err());
}
