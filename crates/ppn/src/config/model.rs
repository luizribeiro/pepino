use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::{Value, json};

use super::path::serialize_path;
use super::{AbsolutePath, DateFormat, Network, NodeId, PlatformDirs, RelayUrl};
use crate::terminal::Color;

pub const DEFAULT_RELAY_URLS: &[&str] = &[
    "https://use1-1.relay.n0.iroh.link.",
    "https://usw1-1.relay.n0.iroh.link.",
    "https://euc1-1.relay.n0.iroh.link.",
    "https://aps1-1.relay.n0.iroh.link.",
];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum Source {
    #[default]
    Default,
    SystemFile {
        #[serde(serialize_with = "serialize_path")]
        path: PathBuf,
    },
    UserFile {
        #[serde(serialize_with = "serialize_path")]
        path: PathBuf,
    },
    ExplicitFile {
        #[serde(serialize_with = "serialize_path")]
        path: PathBuf,
    },
    Env,
    Flag,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Setting<T> {
    pub value: T,
    #[serde(flatten)]
    pub source: Source,
}

impl<T> Setting<T> {
    pub(crate) fn defaulted(value: T) -> Self {
        Self {
            value,
            source: Source::Default,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BootstrapNode {
    pub node_id: NodeId,
    pub relay_url: RelayUrl,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub network: Setting<Network>,
    pub relay_urls: Setting<Vec<RelayUrl>>,
    pub mdns: Setting<bool>,
    pub credentials_file: Setting<AbsolutePath>,
    pub trusted_devices: Setting<Vec<NodeId>>,
    pub bootstrap_nodes: Setting<Vec<BootstrapNode>>,
    pub data_dir: Setting<AbsolutePath>,
    pub ui: UiConfig,
}

#[derive(Clone, Debug)]
pub struct UiConfig {
    pub color: Setting<Color>,
    pub date_format: Setting<DateFormat>,
}

impl Config {
    pub fn defaults(dirs: &PlatformDirs) -> Self {
        Self {
            network: Setting::defaulted("pepino".parse().expect("non-empty default network")),
            relay_urls: Setting::defaulted(
                DEFAULT_RELAY_URLS
                    .iter()
                    .map(|url| {
                        url.parse()
                            .expect("hardcoded default relay is an HTTPS URL")
                    })
                    .collect(),
            ),
            mdns: Setting::defaulted(true),
            credentials_file: Setting::defaulted(dirs.data_dir.join("credentials.cbor")),
            trusted_devices: Setting::defaulted(Vec::new()),
            bootstrap_nodes: Setting::defaulted(Vec::new()),
            data_dir: Setting::defaulted(dirs.data_dir.clone()),
            ui: UiConfig {
                color: Setting::defaulted(Color::Auto),
                date_format: Setting::defaulted(
                    "%Y-%m-%d".parse().expect("valid default date format"),
                ),
            },
        }
    }

    pub fn values(&self) -> BTreeMap<&'static str, Value> {
        BTreeMap::from([
            ("network", json!(self.network)),
            ("relay_urls", json!(self.relay_urls)),
            ("mdns", json!(self.mdns)),
            ("credentials_file", json!(self.credentials_file)),
            ("trusted_devices", json!(self.trusted_devices)),
            ("bootstrap_nodes", json!(self.bootstrap_nodes)),
            ("data_dir", json!(self.data_dir)),
            ("ui.color", json!(self.ui.color)),
            ("ui.date_format", json!(self.ui.date_format)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::config::Platform;

    #[test]
    fn defaults_cover_the_schema_with_default_sources() {
        let dirs =
            PlatformDirs::new(Platform::Linux, Path::new("/home/release"), None, None).unwrap();
        let values = Config::defaults(&dirs).values();
        assert_eq!(values.len(), 9);
        assert!(values.values().all(|value| value["source"] == "default"));
        assert_eq!(values["network"]["value"], "pepino");
        assert_eq!(values["relay_urls"]["value"], json!(DEFAULT_RELAY_URLS));
        assert_eq!(values["mdns"]["value"], true);
        assert_eq!(
            values["credentials_file"]["value"],
            "/home/release/.local/state/pepino/credentials.cbor"
        );
        assert_eq!(values["trusted_devices"]["value"], json!([]));
        assert_eq!(values["bootstrap_nodes"]["value"], json!([]));
        assert_eq!(
            values["data_dir"]["value"],
            "/home/release/.local/state/pepino"
        );
        assert_eq!(values["ui.color"]["value"], "auto");
        assert_eq!(values["ui.date_format"]["value"], "%Y-%m-%d");
    }
}
