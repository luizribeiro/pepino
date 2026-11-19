use std::{fmt, marker::PhantomData, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent, bound = "")]
pub struct Id<Kind>(
    #[serde(with = "serde_bytes")] [u8; 32],
    #[serde(skip)] PhantomData<Kind>,
);

impl<Kind> Id<Kind> {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes, PhantomData)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl<Kind> fmt::Display for Id<Kind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.as_bytes() {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ParseIdError {
    #[error("expected 64 hexadecimal digits")]
    InvalidLength,
    #[error("invalid hexadecimal digit at byte {index}")]
    InvalidDigit { index: usize },
}

impl<Kind> FromStr for Id<Kind> {
    type Err = ParseIdError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut bytes = [0; 32];
        hex::decode_to_slice(text, &mut bytes).map_err(|err| match err {
            hex::FromHexError::InvalidHexCharacter { index, .. } => {
                ParseIdError::InvalidDigit { index }
            }
            hex::FromHexError::OddLength | hex::FromHexError::InvalidStringLength => {
                ParseIdError::InvalidLength
            }
        })?;
        Ok(Self::from_bytes(bytes))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum EventKind {}
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DeviceKind {}
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum IdentityKind {}

pub type EventId = Id<EventKind>;
pub type DeviceId = Id<DeviceKind>;
pub type IdentityId = Id<IdentityKind>;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(pub EventId);

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for TaskId {
    type Err = <EventId as FromStr>::Err;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        text.parse().map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_through_hex_and_serde() {
        let hex = "8a4b507fbdb6c258936a8ebd4f619ada491d4c5a97d38f496b368aca8a67af83";
        let device: DeviceId = hex.to_uppercase().parse().unwrap();
        let identity = IdentityId::from_bytes(*device.as_bytes());
        let task = TaskId(hex.parse().unwrap());
        assert_eq!(device.to_string(), hex);
        assert_eq!(identity.to_string(), hex);
        assert_eq!(task.to_string(), hex);
        assert_eq!(hex.parse::<TaskId>().unwrap(), task);
        let json = serde_json::to_string(&task).unwrap();
        assert_eq!(serde_json::from_str::<TaskId>(&json).unwrap(), task);
    }

    #[test]
    fn rejects_malformed_ids() {
        assert_eq!(
            "8a4b507f".parse::<EventId>(),
            Err(ParseIdError::InvalidLength)
        );
        assert_eq!(
            ("ab".repeat(31) + "cf!").parse::<DeviceId>(),
            Err(ParseIdError::InvalidLength)
        );
        assert_eq!(
            ("ab".repeat(31) + "c!").parse::<DeviceId>(),
            Err(ParseIdError::InvalidDigit { index: 63 })
        );
        assert!(serde_json::from_str::<IdentityId>("[138,75,80,127]").is_err());
    }
}
