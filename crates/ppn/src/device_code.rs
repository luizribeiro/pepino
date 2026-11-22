use data_encoding::BASE32_NOPAD_NOCASE;
use pepino_app::{DEVICE_CODE_PAYLOAD_LIMIT as MAX_PAYLOAD, DeviceRequest};

use crate::error::{CliError, ErrorKind};
use serde::{Serialize, de::DeserializeOwned};

const VERSION: u8 = 2;
const CHECKSUM_LEN: usize = 8;
pub const MAX_CODE_TEXT: usize = 2 * (MAX_PAYLOAD + CHECKSUM_LEN) + "PPN-BOARD-REQUEST-".len();

#[derive(Clone, Copy)]
pub enum Kind {
    Request,
    Grant,
    BoardRequest,
    BoardGrant,
}

impl Kind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Request => "PPN-REQUEST-",
            Self::Grant => "PPN-GRANT-",
            Self::BoardRequest => "PPN-BOARD-REQUEST-",
            Self::BoardGrant => "PPN-BOARD-GRANT-",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Request => "device request code",
            Self::Grant => "device grant code",
            Self::BoardRequest => "board request code",
            Self::BoardGrant => "board grant code",
        }
    }

    fn checksum(self, bytes: &[u8]) -> [u8; CHECKSUM_LEN] {
        let mut hash = blake3::Hasher::new();
        hash.update(self.prefix().as_bytes()).update(bytes);
        let mut checksum = [0; CHECKSUM_LEN];
        hash.finalize_xof().fill(&mut checksum);
        checksum
    }

    pub fn encode(self, value: &impl Serialize) -> Result<String, Error> {
        let mut bytes = vec![VERSION];
        ciborium::into_writer(value, &mut bytes).map_err(|_| Error::Payload)?;
        if bytes.len() > MAX_PAYLOAD {
            return Err(Error::Payload);
        }
        Ok(self.format(bytes))
    }

    fn format(self, mut bytes: Vec<u8>) -> String {
        bytes.extend(self.checksum(&bytes));
        let mut text = self.prefix().to_owned();
        for (index, ch) in BASE32_NOPAD_NOCASE.encode(&bytes).chars().enumerate() {
            if index != 0 && index % 5 == 0 {
                text.push('-');
            }
            text.push(ch);
        }
        text
    }

    pub fn decode<T: DeserializeOwned>(self, text: &str) -> Result<T, Error> {
        let text = text.trim();
        let received = if text.is_empty() {
            "no code"
        } else {
            [
                Self::Request,
                Self::Grant,
                Self::BoardRequest,
                Self::BoardGrant,
            ]
            .into_iter()
            .find(|kind| {
                text.get(..kind.prefix().len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(kind.prefix()))
            })
            .map_or("an unrecognised code", Self::name)
        };
        if received != self.name() {
            return Err(Error::Kind {
                expected: self.name(),
                received,
            });
        }
        if text.len() > MAX_CODE_TEXT {
            return Err(Error::Payload);
        }
        let text = &text[self.prefix().len()..];
        let base32: Vec<_> = text.bytes().filter(|ch| *ch != b'-').collect();
        let bytes = BASE32_NOPAD_NOCASE
            .decode(&base32)
            .map_err(|_| Error::Checksum)?;
        let end = bytes
            .len()
            .checked_sub(CHECKSUM_LEN)
            .filter(|end| *end > 0)
            .ok_or(Error::Checksum)?;
        let (framed, checksum) = bytes.split_at(end);
        if checksum != self.checksum(framed) {
            return Err(Error::Checksum);
        }
        if framed[0] != VERSION {
            return Err(Error::Version);
        }
        if end > MAX_PAYLOAD {
            return Err(Error::Payload);
        }
        let mut payload = &framed[1..];
        let value = ciborium::from_reader(&mut payload).map_err(|_| Error::Payload)?;
        if !payload.is_empty() {
            return Err(Error::Payload);
        }
        Ok(value)
    }
}

#[derive(Serialize, serde::Deserialize)]
struct PackedRequest(
    String,
    #[serde(with = "serde_bytes")] Vec<u8>,
    #[serde(with = "serde_bytes")] [u8; 32],
);

pub fn encode_request(request: &DeviceRequest) -> Result<String, CliError> {
    let info = request.member_info()?;
    let secret = request.secret.ok_or_else(|| invalid(Error::Payload))?;
    Kind::Request
        .encode(&PackedRequest(
            request.name.clone(),
            info.as_bytes().to_vec(),
            secret,
        ))
        .map_err(invalid)
}

pub fn decode_request(code: &str) -> Result<DeviceRequest, CliError> {
    let PackedRequest(name, member, secret) = Kind::Request.decode(code).map_err(invalid)?;
    Ok(DeviceRequest::from_member_bytes(name, &member, secret)?)
}

fn invalid(error: Error) -> CliError {
    CliError::new(ErrorKind::InvalidInput, error.to_string())
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("Expected a {expected}, received {received}.")]
    Kind {
        expected: &'static str,
        received: &'static str,
    },
    #[error("Device code is corrupt or incomplete.")]
    Checksum,
    #[error("Unsupported device code version.")]
    Version,
    #[error("Invalid device code payload.")]
    Payload,
}

#[cfg(test)]
#[test]
fn round_trips_and_detects_wrong_kind_version_and_checksum() {
    let text = Kind::Request.encode(&"Ferrum").unwrap();
    assert_eq!(
        Kind::Request
            .decode::<String>(&text.to_lowercase())
            .unwrap(),
        "Ferrum"
    );
    assert!(matches!(
        Kind::Grant.decode::<String>(&text),
        Err(Error::Kind { .. })
    ));
    assert!(
        Kind::Request
            .decode::<String>(&text[..text.len() - 1])
            .is_err()
    );
    let mut corrupt = text.clone();
    corrupt.replace_range(
        Kind::Request.prefix().len()..=Kind::Request.prefix().len(),
        "B",
    );
    assert_ne!(corrupt, text);
    assert_eq!(
        Kind::Request.decode::<String>(&corrupt),
        Err(Error::Checksum)
    );
    let grant = Kind::Grant.encode(&"Ferrum").unwrap();
    assert!(grant.starts_with("PPN-GRANT-AJTEM-"));
    assert_eq!(Kind::Grant.decode::<String>(&grant).unwrap(), "Ferrum");
    let wrong_domain = grant.replacen("PPN-GRANT-", "PPN-REQUEST-", 1);
    assert_eq!(
        Kind::Request.decode::<String>(&wrong_domain),
        Err(Error::Checksum)
    );
    let version = Kind::Grant.format(vec![1, 0xf6]);
    assert_eq!(Kind::Grant.decode::<String>(&version), Err(Error::Version));
    assert!(text.starts_with("PPN-REQUEST-AJTEM-"));
}

#[cfg(test)]
#[test]
fn device_and_board_codes_have_distinct_types_and_checksums() {
    let invite = Kind::BoardRequest.encode(&"Garden tablet").unwrap();
    assert!(matches!(
        Kind::Request.decode::<String>(&invite),
        Err(Error::Kind { .. })
    ));
    assert!(matches!(
        Kind::BoardGrant.decode::<String>(&invite),
        Err(Error::Kind { .. })
    ));
    assert_eq!(
        Kind::BoardRequest.decode::<String>(&invite).unwrap(),
        "Garden tablet"
    );
    let relabeled = invite.replacen("PPN-BOARD-REQUEST-", "PPN-REQUEST-", 1);
    assert_eq!(
        Kind::Request.decode::<String>(&relabeled),
        Err(Error::Checksum)
    );
}
