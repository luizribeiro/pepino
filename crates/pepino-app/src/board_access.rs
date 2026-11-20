use pepino_core::{BoardId, DeviceId, IdentityId};
use serde::{Deserialize, Serialize};

use crate::{AppError, LocalState, app::invalid};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoardRole {
    Read,
    Write,
    Manage,
}

impl std::fmt::Display for BoardRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Manage => "manage",
        })
    }
}

impl From<BoardRole> for pepino_node::Role {
    fn from(role: BoardRole) -> Self {
        match role {
            BoardRole::Read => Self::Read,
            BoardRole::Write => Self::Write,
            BoardRole::Manage => Self::Manage,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SharedAccess {
    pub owner: IdentityId,
    pub role: BoardRole,
    pub admitted: DeviceId,
}

impl LocalState {
    pub fn shared_access(&self, board: BoardId) -> Option<SharedAccess> {
        self.catalog.shared.get(&board).copied()
    }

    pub fn waiting_for_access(&self, board: BoardId) -> bool {
        self.shared_access(board).is_some_and(|access| {
            access.owner != self.identity_id && access.admitted != self.device_id
        })
    }

    pub fn check_board_write(&self, board: &crate::LocalBoard) -> Result<(), AppError> {
        let denied = |reason: &str| AppError::PermissionDenied {
            path: format!("board {:?}", board.name),
            reason: reason.into(),
        };
        if self.waiting_for_access(board.id) {
            return Err(denied("this device is waiting for access"));
        }
        if self
            .shared_access(board.id)
            .is_some_and(|access| access.role == BoardRole::Read)
        {
            return Err(denied("the read role cannot publish changes"));
        }
        Ok(())
    }

    pub fn check_invitable(&self, board: BoardId) -> Result<(), AppError> {
        if board == self.catalog_board() {
            return Err(invalid(
                "board",
                "this board carries your private catalog. Create another board to share",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[tokio::test]
async fn access_on_another_persons_board_is_device_specific() {
    let directory = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(directory.path(), 0)
        .await
        .unwrap();
    let mut local = app.local_state().clone();
    let house = BoardId::from_bytes([83; 32]);
    assert!(local.check_invitable(local.catalog_board()).is_err());
    assert!(local.check_invitable(house).is_ok());
    local.catalog.shared.insert(
        house,
        SharedAccess {
            owner: IdentityId::from_bytes([84; 32]),
            role: BoardRole::Read,
            admitted: DeviceId::from_bytes([85; 32]),
        },
    );
    assert!(local.waiting_for_access(house));
    local.catalog.shared.get_mut(&house).unwrap().admitted = local.device_id;
    assert!(!local.waiting_for_access(house));
}

#[cfg(test)]
#[tokio::test]
async fn read_members_cannot_enqueue_task_writes() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = crate::write::fixture::open(directory.path(), 0)
        .await
        .unwrap();
    let house = BoardId::from_bytes([86; 32]);
    app.register_created_board(house, "House").await.unwrap();
    app.update_local(move |local| {
        local.catalog.shared.insert(
            house,
            SharedAccess {
                owner: IdentityId::from_bytes([87; 32]),
                role: BoardRole::Read,
                admitted: local.device_id,
            },
        );
    })
    .await
    .unwrap();
    let outcome = app
        .handle_on(
            crate::Request::Write(crate::WriteRequest::ApplyToTasks {
                tasks: vec!["repair window".into()],
                change: crate::TaskChange::Reopen,
            }),
            0,
            Some(("House", "board")),
            false,
        )
        .await;
    assert!(
        matches!(outcome, Err(AppError::PermissionDenied { path, reason }) if path.contains("House") && reason.contains("read role"))
    );
}
