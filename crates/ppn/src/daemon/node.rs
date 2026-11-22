use pepino_app::OpenOptions;
use pepino_core::DeviceId;
use pepino_node::{BootstrapNode, DeviceCredentials, MdnsMode, NodeSettings};

use crate::{
    config::Config,
    error::{CliError, ErrorKind},
};

pub async fn settings(config: &Config) -> Result<NodeSettings, CliError> {
    let data = config.data_dir.value.as_path();
    let path = config.credentials_file.value.as_path();
    let options = OpenOptions {
        credentials_file: Some(path.to_owned()),
        ..OpenOptions::default()
    };
    let app = pepino_app::App::open_with_options(data, pepino_app::wall_time_ms(), options).await?;
    let credentials = DeviceCredentials::load(path).map_err(|_| {
        CliError::new(
            ErrorKind::Failure,
            "Could not load the configured device credentials.",
        )
    })?;
    if credentials.device_id() != app.local_state().device_id {
        return Err(CliError::new(
            ErrorKind::Failure,
            "The configured credentials do not match the local device.",
        ));
    }
    let mut settings = common_settings(config, credentials, "node.sqlite");
    let local = app.local_state();
    for peer in local.peer_hint().into_iter().chain(local.shared_peers()) {
        if let Some(relay_url) = peer.relay_url
            && !settings
                .bootstrap_nodes
                .iter()
                .any(|entry| entry.device_id == peer.id && entry.relay_url == relay_url)
        {
            settings.bootstrap_nodes.push(BootstrapNode {
                device_id: peer.id,
                relay_url,
            });
        }
    }
    Ok(settings)
}

pub(super) fn relay_settings(config: &Config) -> Result<NodeSettings, CliError> {
    let keys = pepino_app::relay_credentials(
        config.data_dir.value.as_path(),
        config.credentials_file.value.as_path(),
    )?;
    let mut settings = common_settings(config, keys, "relay.sqlite");
    settings.relay_urls.truncate(1);
    Ok(settings)
}

fn common_settings(config: &Config, keys: DeviceCredentials, database: &str) -> NodeSettings {
    let mut settings = NodeSettings::new(keys, config.data_dir.value.as_path().join(database));
    settings.network_name = config.network.value.as_str().to_owned();
    settings.relay_urls = config
        .relay_urls
        .value
        .iter()
        .map(|relay| relay.as_url().as_str().to_owned())
        .collect();
    settings.bootstrap_nodes = config
        .bootstrap_nodes
        .value
        .iter()
        .map(|entry| BootstrapNode {
            device_id: DeviceId::from_bytes(*entry.node_id.as_bytes()),
            relay_url: entry.relay_url.as_url().as_str().to_owned(),
        })
        .collect();
    settings.mdns = if config.mdns.value {
        MdnsMode::Active
    } else {
        MdnsMode::Disabled
    };
    settings.bind_port = config.port.value;
    #[cfg(pepino_network_tests)]
    if std::env::var_os("PPN_TEST_LOOPBACK").is_some() {
        settings.bind_ip_v4 = Some(std::net::Ipv4Addr::LOCALHOST);
        settings.bind_ip_v6 = Some(std::net::Ipv6Addr::LOCALHOST);
    }
    settings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Platform, PlatformDirs};
    use std::path::Path;

    #[tokio::test]
    async fn an_uninitialized_directory_does_not_create_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let platform = PlatformDirs::new(Platform::Linux, dir.path(), None, None);
        let config = crate::config::validate_file(
            Path::new("/etc/release.toml"),
            "data_dir = '~/release'",
            &platform,
        )
        .unwrap();
        assert_eq!(
            settings(&config).await.err().unwrap().kind(),
            ErrorKind::NotInitialized
        );
        assert!(!config.data_dir.value.as_path().exists());
    }
}
