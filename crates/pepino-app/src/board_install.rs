use pepino_core::BoardId;
use serde::{Deserialize, Serialize};

use crate::{
    App, AppError, BoardGrant, BoardJoinState, LocalState, PeerHint, SharedAccess, SignedGrant,
    app::{invalid, lock},
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

    pub(crate) fn install_share(
        &mut self,
        grant: &BoardGrant,
        receipt: String,
        now: u64,
    ) -> Result<(), AppError> {
        let id = grant.board.id;
        self.check_invitable(id)?;
        if self.board_deletion_pending(id) {
            return Err(invalid(
                "grant",
                "deleting the previous local history is still pending",
            ));
        }
        if self
            .catalog
            .board_request
            .as_ref()
            .is_none_or(|request| request.identity.key() != self.own_person_key())
            || grant.recipient != self.device_id
            || (grant.owner.id == self.identity_id
                && (grant.owner.key() != self.own_person_key()
                    || (self.shared_access(id).is_none() && !self.board_forgotten(id))))
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
        let existing = self.shared_access(id);
        if self.boards().iter().any(|board| board.id == id)
            && existing.is_none_or(|access| {
                access.owner != grant.owner.id
                    || access
                        .owner_root
                        .is_some_and(|root| root != grant.owner.root)
            })
        {
            return Err(invalid(
                "grant",
                "the board grant conflicts with the board's pinned owner",
            ));
        }
        self.catalog.shared.insert(
            id,
            SharedAccess {
                owner: grant.owner.id,
                owner_root: Some(grant.owner.root),
                role: grant.role,
                admitted: self.device_id,
            },
        );
        let forgotten = self.board_forgotten(id);
        self.change_board_presence(id, false, now)?;
        self.name_board(id, &grant.board.name);
        if existing.is_some() || forgotten {
            self.catalog.observed_board_removals.remove(&id);
            self.catalog
                .card_exchanges
                .retain(|_, route| route.board != id);
            self.catalog.wait_for_board_join(id);
            self.catalog.readmission_replays.insert(id);
        }
        if let Some(channel) = grant.card_exchange {
            self.add_card_exchange(
                channel,
                crate::CardExchange {
                    board: id,
                    person: self.identity_id,
                    root: Some(self.own_person_key().1),
                    incarnation: None,
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
        let guard = lock(&self.directory, self.lock_timeout).await?;
        let local = self.fresh_local_state().await?;
        local.check_device_write()?;
        if local
            .catalog
            .joined_shares
            .get(&board)
            .is_some_and(|saved| saved.receipt.as_ref() == Some(&receipt))
        {
            return Ok((board, peer, local.board_join_state(board)));
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
        let (mut local, guard) = self.resume_board_history(local, guard).await?;
        local.install_share(&grant, receipt, crate::wall_time_ms())?;
        let (local, _guard) = self.save_cleanup(local, guard).await?;
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
    local
        .install_share(&grant, "grant digest".into(), 1000)
        .unwrap();
    assert!(local.catalog.board_request.is_none());
    assert_eq!(local.boards().len(), 2);
    assert_eq!(
        local.shared_access(id).unwrap().owner_root,
        Some(grant.owner.root)
    );
    assert_eq!(local.catalog_board(), private);
    assert_eq!(local.card_exchanges().values().next().unwrap().board, id);
    assert_eq!(local.board_join_state(id), BoardJoinState::WaitingToBeAdded);
    assert!(local.shared_peers().next() == Some(grant.peer.clone()));
    grant.board.id = BoardId::from_bytes([85; 32]);
    assert!(
        local
            .install_share(&grant, "other digest".into(), 1000)
            .is_err()
    );
}

#[cfg(test)]
#[tokio::test]
async fn a_new_grant_finishes_pending_deletion_before_accepting_writes() {
    use pepino_core::{Envelope, Event, Hlc, InitialFields};
    use pepino_node::{Node, NodeSettings};
    use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

    let dir = tempfile::tempdir().unwrap();
    let owner = App::init(dir.path().join("owner"), 0).await.unwrap();
    let owner_keys = owner.identity_credentials().unwrap();
    let owner_card = owner
        .confirm_roster_device()
        .await
        .unwrap()
        .identity_card()
        .unwrap();
    let path = dir.path().join("tablet");
    let mut guest = App::init(&path, 0).await.unwrap();
    let node = Node::spawn(NodeSettings::offline(
        guest.identity_credentials().unwrap(),
        path.join("node.sqlite"),
    ))
    .await
    .unwrap();
    let member = node.member_info().await.unwrap();
    let board = BoardId::from_bytes([89; 32]);
    let issue = |request: &crate::BoardRequest| {
        BoardGrant {
            recipient: node.device_id(),
            request: request.binding().unwrap(),
            board: crate::LocalBoard {
                id: board,
                name: "Workshop".into(),
            },
            role: crate::BoardRole::Write,
            card_exchange: None,
            owner: owner_card.clone(),
            peer: PeerHint {
                id: owner_keys.device_id(),
                relay_url: None,
            },
        }
        .issue(request, &owner_keys)
        .unwrap()
    };
    let first = guest.request_board(&member, None).await.unwrap();
    guest.join_board(issue(&first)).await.unwrap();
    let event = Envelope::new(
        node.device_id(),
        Hlc {
            wall_ms: 1000,
            counter: 0,
        },
        Event::TaskCreated {
            title: "Repair the old bench vice".into(),
            fields: InitialFields::default(),
        },
    )
    .unwrap();
    guest
        .store()
        .append(board, &event.encode().unwrap(), Some(node.device_id()))
        .await
        .unwrap();
    let second = guest.request_board(&member, None).await.unwrap();

    let mut local = guest.fresh_local_state().await.unwrap();
    local
        .change_board_presence(board, true, crate::wall_time_ms())
        .unwrap();
    local.forget_board(board, crate::wall_time_ms());
    local.replace(&path).unwrap();
    assert!(local.board_deletion_pending(board));
    assert!(local.resolve_board("Workshop", "board").is_err());
    assert_eq!(guest.store().statistics(board).await.unwrap().total, 1);

    let options = SqliteConnectOptions::new().filename(path.join("pepino.sqlite"));
    let mut db = SqliteConnection::connect_with(&options).await.unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_board_deletion BEFORE DELETE ON event_log
         BEGIN SELECT RAISE(FAIL, 'blocked by test'); END",
    )
    .execute(&mut db)
    .await
    .unwrap();
    assert!(matches!(guest.join_board(issue(&second)).await,
        Err(AppError::Storage { reason, .. }) if reason.contains("blocked by test")));
    let local = guest.fresh_local_state().await.unwrap();
    assert!(local.board_deletion_pending(board));
    assert!(local.resolve_board("Workshop", "board").is_err());
    sqlx::query("DROP TRIGGER fail_board_deletion")
        .execute(&mut db)
        .await
        .unwrap();
    db.close().await.unwrap();

    guest.join_board(issue(&second)).await.unwrap();
    let local = guest.fresh_local_state().await.unwrap();
    assert!(local.resolve_board("Workshop", "board").is_ok());
    assert!(!local.board_deletion_pending(board));
    assert_eq!(guest.store().statistics(board).await.unwrap().total, 0);
    guest.record_readmission_replay(board, false).await.unwrap();

    let write = serde_json::from_value(serde_json::json!({
        "op": "add_task", "title": "Order the new bench vice"
    }))
    .unwrap();
    guest
        .handle_on(
            write,
            crate::wall_time_ms(),
            Some(("Workshop", "--board")),
            false,
        )
        .await
        .unwrap();
    drop(guest);
    let guest = App::open(path, crate::wall_time_ms()).await.unwrap();
    let tasks = guest.store().tasks(board).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "Order the new bench vice");
    node.shutdown().await.unwrap();
}
