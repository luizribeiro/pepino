use chrono::{DateTime, Utc};
use pepino_core::{BoardId, DeviceId, EventId};
use pepino_node::DeviceCredentials;
use serde::{Deserialize, Serialize};

use crate::{App, AppError, LocalState, MembershipAction, PersonKey, WireFormat, app::invalid};

const SIGNATURE_SCOPE: &[u8] = b"pepino signed board leave v1\0";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoardLeave {
    pub board: BoardId,
    pub person: PersonKey,
    pub device: DeviceId,
    pub at: DateTime<Utc>,
    pub incarnation: Option<EventId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SignedBoardLeave {
    pub leave: BoardLeave,
    signature: Vec<u8>,
}

impl BoardLeave {
    fn signing_bytes(&self) -> Result<Vec<u8>, AppError> {
        let mut bytes = SIGNATURE_SCOPE.to_vec();
        bytes.extend(WireFormat::Cbor.encode(self)?);
        Ok(bytes)
    }

    fn sign(self, keys: &DeviceCredentials) -> Result<SignedBoardLeave, AppError> {
        let signature = keys.sign(&self.signing_bytes()?).to_vec();
        Ok(SignedBoardLeave {
            leave: self,
            signature,
        })
    }
}

impl SignedBoardLeave {
    pub fn verify(&self) -> Result<bool, AppError> {
        let Ok(signature) = self.signature.as_slice().try_into() else {
            return Ok(false);
        };
        Ok(DeviceCredentials::verify(
            self.leave.device,
            &self.leave.signing_bytes()?,
            signature,
        ))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoardDeparture {
    Leaving,
    Left,
    Removed,
}

impl std::fmt::Display for BoardDeparture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Leaving => "leaving (waiting for a manager)",
            Self::Left => "left",
            Self::Removed => "removed",
        })
    }
}

impl LocalState {
    pub fn board_departure(&self, board: BoardId) -> Option<BoardDeparture> {
        let latest = self.latest_membership(board, self.own_person_key());
        if let Some(record) = latest {
            match record.change.action {
                MembershipAction::Removed => return Some(BoardDeparture::Removed),
                MembershipAction::Left => return Some(BoardDeparture::Left),
                _ => (),
            }
        }
        self.board_leave_requests(board)
            .any(|request| request.leave.person == self.own_person_key())
            .then_some(BoardDeparture::Leaving)
    }

    pub fn board_departure_confirmed(&self, board: BoardId) -> bool {
        self.latest_membership(board, self.own_person_key())
            .is_some_and(|record| {
                record.change.action.role().is_none()
                    && (record.applied || self.catalog.observed_board_removals.contains(&board))
            })
    }

    pub fn board_leave_requests(&self, board: BoardId) -> impl Iterator<Item = &SignedBoardLeave> {
        self.catalog
            .board_leaves
            .get(&board)
            .into_iter()
            .flat_map(|requests| requests.values())
            .filter(move |request| {
                request.leave.incarnation
                    == self.membership_incarnation(board, request.leave.person)
                    && self.active_board_device(board, request.leave.person, request.leave.device)
            })
    }
    pub(crate) fn save_board_leave(&mut self, board: BoardId, request: SignedBoardLeave) {
        let mut key = crate::people_names::person_storage_key(request.leave.person);
        if let Some(incarnation) = request.leave.incarnation {
            key.push('/');
            key.push_str(&incarnation.to_string());
        }
        let requests = self.catalog.board_leaves.entry(board).or_default();
        match requests.entry(key) {
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let current = entry.get();
                if (request.leave.at, request.leave.device)
                    > (current.leave.at, current.leave.device)
                {
                    entry.insert(request);
                }
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(request);
            }
        }
    }
}

impl App {
    pub async fn request_board_leave(&self, board: BoardId) -> Result<SignedBoardLeave, AppError> {
        let keys = self.identity_credentials()?;
        let local = self.fresh_local_state().await?;
        local.check_device_write()?;
        local.check_invitable(board)?;
        let person = local.own_person_key();
        let request = BoardLeave {
            board,
            person,
            device: keys.device_id(),
            at: std::time::SystemTime::now().into(),
            incarnation: local.membership_incarnation(board, person),
        }
        .sign(&keys)?;
        self.record_board_leave(board, request.clone()).await?;
        Ok(request)
    }

    pub async fn record_board_leave(
        &self,
        board: BoardId,
        request: SignedBoardLeave,
    ) -> Result<(), AppError> {
        if board != request.leave.board || !request.verify()? {
            return Err(invalid("person", "invalid board leave signature"));
        }
        self.update_local(move |local| {
            if local.board_people(board).any(|person| {
                person.key() == request.leave.person
                    && person.devices().any(|device| {
                        device.id == request.leave.device
                            && person.confirms(device.id)
                            && device.state == crate::RosterState::Active
                    })
            }) || (local.own_person_key() == request.leave.person
                && local.roster_device_confirmed(request.leave.device)
                && local.active_roster_device(request.leave.device))
            {
                local.save_board_leave(board, request);
            }
        })
        .await
    }

    pub async fn observe_board_removal(
        &self,
        board: BoardId,
        removed: bool,
    ) -> Result<(), AppError> {
        self.update_local(move |local| {
            if removed {
                local.catalog.observed_board_removals.insert(board);
            } else {
                local.catalog.observed_board_removals.remove(&board);
            }
        })
        .await
    }
}
