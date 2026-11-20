use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::app::invalid;
use crate::state::storage;
use crate::{AppError, Request, Response};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WireRequest {
    pub v: u32,
    pub request: Request,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all_boards: bool,
}

impl WireRequest {
    pub const fn new(request: Request) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            request,
            board: None,
            all_boards: false,
        }
    }

    pub async fn handle(self, app: &mut crate::App, now: u64) -> WireResponse {
        let board = self.board.as_deref().map(|board| (board, "board"));
        WireResponse::new(
            app.handle_on(self.request, now, board, self.all_boards)
                .await,
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WireResponse {
    pub v: u32,
    pub result: WireResult,
}

impl WireResponse {
    pub fn new(result: Result<Response, AppError>) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            result: match result {
                Ok(response) => WireResult::Ok(response),
                Err(error) => WireResult::Error(error),
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WireResult {
    Ok(Response),
    Error(AppError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireFormat {
    Json,
    Cbor,
}

impl WireFormat {
    /// Encodes a [`WireRequest`] or [`WireResponse`] in this format.
    pub fn encode(self, message: &impl Serialize) -> Result<Vec<u8>, AppError> {
        match self {
            Self::Json => serde_json::to_vec(message)
                .map_err(|error| storage("encode JSON message", error, false)),
            Self::Cbor => {
                let mut bytes = Vec::new();
                ciborium::into_writer(message, &mut bytes)
                    .map_err(|error| storage("encode CBOR message", error, false))?;
                Ok(bytes)
            }
        }
    }

    /// Decodes a [`WireRequest`] or [`WireResponse`], checking its version before its body.
    pub fn decode<T: DeserializeOwned>(self, bytes: &[u8]) -> Result<T, AppError> {
        #[derive(Deserialize)]
        struct Version {
            v: u32,
        }

        let Version { v } = self.decode_value(bytes)?;
        if v != PROTOCOL_VERSION {
            return Err(AppError::UnsupportedVersion {
                requested: v,
                supported: PROTOCOL_VERSION,
            });
        }
        self.decode_value(bytes)
    }

    fn decode_value<T: DeserializeOwned>(self, mut bytes: &[u8]) -> Result<T, AppError> {
        match self {
            Self::Json => serde_json::from_slice(bytes).map_err(|error| invalid("request", error)),
            Self::Cbor => {
                let value =
                    ciborium::from_reader(&mut bytes).map_err(|error| invalid("request", error))?;
                if !bytes.is_empty() {
                    return Err(invalid("request", "unexpected bytes after CBOR message"));
                }
                Ok(value)
            }
        }
    }
}

#[cfg(test)]
#[test]
fn routing_is_optional_in_both_wire_formats() {
    let routed =
        serde_json::json!({"v":1,"board":"Workshop","all_boards":true,"request":{"op":"status"}});
    for format in [WireFormat::Json, WireFormat::Cbor] {
        let request: WireRequest = format.decode(&format.encode(&routed).unwrap()).unwrap();
        assert_eq!(request.board.as_deref(), Some("Workshop"));
        assert!(request.all_boards);
    }
    let request: WireRequest = WireFormat::Json
        .decode(br#"{"v":1,"request":{"op":"status"}}"#)
        .unwrap();
    assert!(request.board.is_none());
    assert!(!request.all_boards);
}
