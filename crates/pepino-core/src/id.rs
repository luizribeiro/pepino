use std::{fmt, marker::PhantomData, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Id<Kind>([u8; 32], PhantomData<Kind>);

impl<Kind> Serialize for Id<Kind> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            serializer.collect_str(self)
        } else {
            serializer.serialize_bytes(self.as_bytes())
        }
    }
}

impl<'de, Kind> Deserialize<'de> for Id<Kind> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            String::deserialize(deserializer)?
                .parse()
                .map_err(de::Error::custom)
        } else {
            serde_bytes::deserialize(deserializer).map(Self::from_bytes)
        }
    }
}

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

impl<Kind> fmt::Debug for Id<Kind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
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

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(pub EventId);

impl TaskId {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(EventId::from_bytes(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

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
        let task = TaskId::from_bytes(*device.as_bytes());
        assert_eq!(task.as_bytes(), device.as_bytes());
        assert_eq!(device.to_string(), hex);
        assert_eq!(identity.to_string(), hex);
        assert_eq!(task.to_string(), hex);
        assert_eq!(format!("{device:?}"), hex);
        assert_eq!(format!("{identity:?}"), hex);
        assert_eq!(format!("{task:?}"), hex);
        assert_eq!(format!("{:?}", task.0), hex);
        assert_eq!(hex.parse::<TaskId>().unwrap(), task);
        let json = format!(r#""{hex}""#);
        assert_json(&json, device);
        assert_json(&json, identity);
        assert_json(&json, task);
        assert_json(&json, task.0);

        let expected = [&[0x58, 0x20][..], device.as_bytes()].concat();
        assert_cbor(&expected, device);
        assert_cbor(&expected, identity);
        assert_cbor(&expected, task);
        assert_cbor(&expected, task.0);
    }

    fn assert_json<T>(expected: &str, id: T)
    where
        T: Serialize + for<'de> Deserialize<'de> + fmt::Debug + PartialEq,
    {
        assert_eq!(serde_json::to_string(&id).unwrap(), expected);
        assert_eq!(serde_json::from_str::<T>(expected).unwrap(), id);
    }

    fn assert_cbor<T>(expected: &[u8], id: T)
    where
        T: Serialize + for<'de> Deserialize<'de> + fmt::Debug + PartialEq,
    {
        let mut bytes = Vec::new();
        ciborium::into_writer(&id, &mut bytes).unwrap();
        assert_eq!(bytes, expected);
        assert_eq!(ciborium::from_reader::<T, _>(expected).unwrap(), id);
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
        assert!(ciborium::from_reader::<EventId, _>(&[0x41, 0x89][..]).is_err());
    }
}
