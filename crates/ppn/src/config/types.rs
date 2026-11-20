use std::str::FromStr;

use chrono::format::{Item, StrftimeItems};
use serde::{Serialize, Serializer};
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ValueError(pub(crate) &'static str);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Network(String);

impl Network {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Network {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.trim().is_empty() {
            return Err(ValueError("expected a non-empty network name"));
        }
        Ok(Self(value.to_owned()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct DateFormat(String);

impl DateFormat {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for DateFormat {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if StrftimeItems::new(value).any(|item| item == Item::Error) {
            return Err(ValueError("expected a valid strftime format"));
        }
        Ok(Self(value.to_owned()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeId([u8; 32]);

impl NodeId {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl FromStr for NodeId {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut bytes = [0; 32];
        hex::decode_to_slice(value, &mut bytes)
            .map_err(|_| ValueError("expected a node ID with 64 hex digits"))?;
        Ok(Self(bytes))
    }
}

impl Serialize for NodeId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(self.0))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct RelayUrl {
    text: String,
    #[serde(skip)]
    url: Url,
}

impl RelayUrl {
    pub fn as_url(&self) -> &Url {
        &self.url
    }
}

impl FromStr for RelayUrl {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let error = ValueError("expected an HTTPS URL with a host");
        let (scheme, authority) = value.split_once("://").ok_or(error)?;
        if !scheme.eq_ignore_ascii_case("https")
            || authority.starts_with(['/', '\\'])
            || value.chars().any(char::is_whitespace)
        {
            return Err(error);
        }
        let url = Url::parse(value).map_err(|_| error)?;
        Ok(Self {
            text: value.to_owned(),
            url,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_network_and_date_format() {
        assert_eq!(
            "release-team".parse::<Network>().unwrap().as_str(),
            "release-team"
        );
        assert!("   ".parse::<Network>().is_err());
        assert_eq!(
            "%a %Y-%m-%d".parse::<DateFormat>().unwrap().as_str(),
            "%a %Y-%m-%d"
        );
        for invalid in ["%Y-%Q", "%"] {
            assert!(invalid.parse::<DateFormat>().is_err());
        }
    }

    #[test]
    fn validates_node_ids_and_relay_urls() {
        let id = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        assert_eq!(id.parse::<NodeId>().unwrap().as_bytes()[15], 255);
        for invalid in [&id[..63], &format!("z{}", &id[1..])] {
            assert!(invalid.parse::<NodeId>().is_err());
        }
        let relay = "https://relay.release.example:8443/path";
        assert_eq!(
            relay.parse::<RelayUrl>().unwrap().as_url().port(),
            Some(8443)
        );
        for invalid in [
            "http://relay.release.example",
            "https://",
            "https:///relay.example",
            "relay.example",
        ] {
            assert!(invalid.parse::<RelayUrl>().is_err());
        }
    }
}
