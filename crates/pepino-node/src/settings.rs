use std::{net::Ipv4Addr, path::PathBuf, time::Duration};

use p2panda::{
    Hash, NodeBuilder, RelayUrl, VerifyingKey, network::MdnsDiscoveryMode, node::AckPolicy,
};
use pepino_core::DeviceId;

use crate::{DeviceCredentials, Error};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MdnsMode {
    Disabled,
    #[default]
    Active,
    Passive,
}

#[derive(Clone, Debug)]
pub struct BootstrapNode {
    pub device_id: DeviceId,
    pub relay_url: String,
}

#[derive(Clone)]
pub struct NodeSettings {
    pub network_name: String,
    pub relay_urls: Vec<String>,
    pub bootstrap_nodes: Vec<BootstrapNode>,
    pub mdns: MdnsMode,
    pub bind_ip_v4: Option<Ipv4Addr>,
    pub bind_port: Option<u16>,
    pub credentials: DeviceCredentials,
    pub database_path: PathBuf,
    pub publish_confirmation_timeout: Duration,
}

impl NodeSettings {
    pub fn new(credentials: DeviceCredentials, database_path: impl Into<PathBuf>) -> Self {
        Self {
            network_name: "pepino".into(),
            relay_urls: Vec::new(),
            bootstrap_nodes: Vec::new(),
            mdns: MdnsMode::Active,
            bind_ip_v4: None,
            bind_port: None,
            credentials,
            database_path: database_path.into(),
            publish_confirmation_timeout: Duration::from_secs(30),
        }
    }

    pub fn validate(&self) -> Result<(), Error> {
        self.builder().map(|_| ())
    }

    pub(crate) fn builder(&self) -> Result<NodeBuilder, Error> {
        if self.network_name.trim().is_empty() || self.database_path.file_name().is_none() {
            return Err(Error::Settings(
                "a network name and database filename are required".into(),
            ));
        }
        let mut builder = p2panda::builder()
            .credentials(self.credentials.0.clone())
            .network_id(Hash::digest(self.network_name.as_bytes()).into())
            .ack_policy(AckPolicy::Explicit)
            .mdns_mode(match self.mdns {
                MdnsMode::Disabled => MdnsDiscoveryMode::Disabled,
                MdnsMode::Active => MdnsDiscoveryMode::Active,
                MdnsMode::Passive => MdnsDiscoveryMode::Passive,
            });
        for url in &self.relay_urls {
            builder = builder.relay_url(relay_url(url)?);
        }
        for peer in &self.bootstrap_nodes {
            let key = VerifyingKey::from_bytes(peer.device_id.as_bytes()).map_err(|error| {
                Error::Settings(format!("bootstrap device {}: {error}", peer.device_id))
            })?;
            builder = builder.bootstrap(key, relay_url(&peer.relay_url)?);
        }
        if let Some(ip) = self.bind_ip_v4 {
            builder = builder.bind_ip_v4(ip);
        }
        if let Some(port) = self.bind_port {
            builder = builder.bind_port_v4(port);
        }
        Ok(builder)
    }
}

fn relay_url(text: &str) -> Result<RelayUrl, Error> {
    let url: RelayUrl = text
        .parse()
        .map_err(|_| Error::Settings("invalid relay URL".into()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err(Error::Settings(
            "relay URLs require HTTP or HTTPS and a host".into(),
        ));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_network_and_bootstrap_settings_without_starting_a_node() {
        let dir = tempfile::tempdir().unwrap();
        let keys = DeviceCredentials::load_or_create(dir.path().join("device.cbor")).unwrap();
        let mut settings = NodeSettings::new(keys.clone(), dir.path().join("node.sqlite"));
        settings.bootstrap_nodes.push(BootstrapNode {
            device_id: keys.device_id(),
            relay_url: "https://relay.example.org".into(),
        });
        assert!(settings.validate().is_ok());
        settings
            .relay_urls
            .push("udp://relay.example.org:4242".into());
        assert!(matches!(settings.validate(), Err(Error::Settings(_))));
        settings.relay_urls.clear();
        settings.network_name.clear();
        assert!(matches!(settings.validate(), Err(Error::Settings(_))));
        settings.network_name = "Release engineering".into();
        settings.database_path.clear();
        assert!(matches!(settings.validate(), Err(Error::Settings(_))));
    }
}
