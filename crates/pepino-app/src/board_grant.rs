use pepino_core::DeviceId;
use pepino_node::DeviceCredentials;
use serde::{Deserialize, Serialize};

use crate::{
    AppError, BoardRequest, BoardRole, IdentityCard, LocalBoard, PeerHint, SignedGrant,
    app::invalid,
};

pub const DEVICE_CODE_PAYLOAD_LIMIT: usize = 64 * 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoardGrant {
    pub recipient: DeviceId,
    pub request: String,
    pub board: LocalBoard,
    pub role: BoardRole,
    pub owner: IdentityCard,
    pub peer: PeerHint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_exchange: Option<pepino_core::BoardId>,
}

impl std::fmt::Debug for BoardGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoardGrant").finish_non_exhaustive()
    }
}

impl BoardGrant {
    pub fn check_code_size(&self) -> Result<(), AppError> {
        if SignedGrant::encoded_size(self)? >= DEVICE_CODE_PAYLOAD_LIMIT {
            return Err(invalid("grant", "the board grant is too large to encode"));
        }
        Ok(())
    }

    pub fn issue(
        &self,
        request: &BoardRequest,
        keys: &DeviceCredentials,
    ) -> Result<SignedGrant, AppError> {
        self.check_request(request)?;
        if self.peer.id != keys.device_id() {
            return Err(invalid(
                "grant",
                "the issuing credentials do not match the grant",
            ));
        }
        SignedGrant::issue_body(self, request.secret()?, keys)
    }

    pub fn verify(signed: SignedGrant, request: &BoardRequest) -> Result<Self, AppError> {
        let grant = signed.verify_body(request.secret()?, |body: &Self| body.peer.id)?;
        grant.check_request(request)?;
        grant.owner.verified_record()?;
        crate::App::validate_board_name(&grant.board.name)?;
        if grant.card_exchange.is_some_and(|id| id == grant.board.id) {
            return Err(invalid("grant", "invalid identity update channel"));
        }
        if let Some(url) = grant.peer.relay_url.as_deref() {
            pepino_node::peer_relay_url(url)
                .map_err(|_| invalid("grant", "invalid granting device relay URL"))?;
        }
        Ok(grant)
    }

    fn check_request(&self, request: &BoardRequest) -> Result<(), AppError> {
        if self.recipient != request.device.id
            || self.request != request.binding()?
            || self.recipient == self.peer.id
        {
            return Err(invalid(
                "grant",
                "this grant does not match this board request",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[tokio::test]
async fn board_grants_are_bound_to_the_original_request() {
    let directory = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(directory.path(), 0)
        .await
        .unwrap();
    let keys = app.identity_credentials().unwrap();
    let local = app
        .reconcile_identity(keys.device_id(), &keys)
        .await
        .unwrap();
    let mut request = BoardRequest {
        device: crate::DeviceRequest {
            id: DeviceId::from_bytes([86; 32]),
            name: "Workshop tablet".into(),
            member_info: "test member bundle".into(),
            secret: Some([87; 32]),
        },
        identity: local.identity_card().unwrap(),
        issuance: None,
    };
    let grant = BoardGrant {
        recipient: request.device.id,
        request: request.binding().unwrap(),
        board: LocalBoard {
            id: pepino_core::BoardId::from_bytes([88; 32]),
            name: "Workshop".into(),
        },
        role: BoardRole::Write,
        card_exchange: None,
        owner: request.identity.clone(),
        peer: PeerHint {
            id: keys.device_id(),
            relay_url: None,
        },
    };
    assert!(grant.check_code_size().is_ok());
    let oversized = BoardGrant {
        peer: PeerHint {
            relay_url: Some("x".repeat(DEVICE_CODE_PAYLOAD_LIMIT)),
            ..grant.peer.clone()
        },
        ..grant.clone()
    };
    assert!(oversized.check_code_size().is_err());
    let signed = grant.issue(&request, &keys).unwrap();
    let other = grant.issue(&request, &keys).unwrap();
    assert_eq!(
        BoardGrant::verify(signed, &request).unwrap().board,
        grant.board
    );
    request.device.name = "Garden tablet".into();
    assert!(BoardGrant::verify(other, &request).is_err());
}
