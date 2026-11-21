use std::path::Path;
use std::str::FromStr;

use clap::ValueEnum;

use super::diagnostics::invalid_config;
use super::document::{Input, Validator, text};
use super::model::{Draft, Key, ROOT_KEYS, TABLES};
use super::{AbsolutePath, BootstrapNode, Config, PlatformDirs, Source, ValueError};
use crate::error::CliError;
use crate::terminal::Color;

pub fn validate_file(path: &Path, text: &str, dirs: &PlatformDirs) -> Result<Config, CliError> {
    let mut draft = Draft::new(dirs);
    draft.apply_file(
        path,
        text,
        &Source::ExplicitFile {
            path: path.to_owned(),
        },
    )?;
    draft.finish().map_err(default_error)
}

impl Draft<'_> {
    pub fn apply_file(&mut self, path: &Path, text: &str, source: &Source) -> Result<(), CliError> {
        let mut validator = Validator::new(Some((path, text)));
        for (key, value) in validator.document(text) {
            let name = key.get_ref().as_ref();
            if let Some((_, keys)) = TABLES.iter().find(|(table, _)| *table == name) {
                let table = value
                    .get_ref()
                    .as_table()
                    .ok_or(ValueError("expected a table"));
                if let Some(table) = validator.require(name, &value, table) {
                    for (key, value) in table {
                        let name = format!("{name}.{}", key.get_ref());
                        if let Some(entry) = find_key(keys, &name) {
                            (entry.apply)(self, value, source, &mut validator);
                        } else {
                            validator.unknown(&name, key.span(), names(keys));
                        }
                    }
                }
            } else if let Some(entry) = find_key(ROOT_KEYS, name) {
                (entry.apply)(self, &value, source, &mut validator);
            } else {
                validator.unknown(
                    name,
                    key.span(),
                    names(ROOT_KEYS).chain(TABLES.iter().map(|(table, _)| *table)),
                );
            }
        }
        invalid_config(validator.problems)
    }
}

fn find_key<'key>(entries: &'key [Key], name: &str) -> Option<&'key Key> {
    entries.iter().find(|entry| entry.environment.key == name)
}

fn names(entries: &[Key]) -> impl Iterator<Item = &'static str> {
    entries.iter().map(|entry| entry.environment.key)
}

pub(crate) fn scalar<T: FromStr<Err = ValueError>>(
    validator: &mut Validator<'_>,
    key: &str,
    input: &Input<'_>,
) -> Option<T> {
    validator.parse(key, input)
}

pub(crate) fn boolean(validator: &mut Validator<'_>, key: &str, input: &Input<'_>) -> Option<bool> {
    validator.require(
        key,
        input,
        input
            .get_ref()
            .as_bool()
            .ok_or(ValueError("expected a boolean")),
    )
}

pub(crate) fn port(
    validator: &mut Validator<'_>,
    key: &str,
    input: &Input<'_>,
) -> Option<Option<u16>> {
    let port = input
        .get_ref()
        .as_integer()
        .and_then(|value| u16::from_str_radix(value.as_str(), value.radix()).ok())
        .filter(|port| *port != 0)
        .ok_or(ValueError("expected an integer from 1 to 65535"));
    validator.require(key, input, port).map(Some)
}

pub(crate) fn max_boards(
    validator: &mut Validator<'_>,
    key: &str,
    input: &Input<'_>,
) -> Option<usize> {
    let max = input
        .get_ref()
        .as_integer()
        .and_then(|value| u32::from_str_radix(value.as_str(), value.radix()).ok())
        .ok_or(ValueError("expected an integer from 0 to 4294967295"));
    validator.require(key, input, max).map(|max| max as usize)
}

pub(crate) fn color(validator: &mut Validator<'_>, key: &str, input: &Input<'_>) -> Option<Color> {
    let color = text(input.get_ref()).and_then(|value| {
        Color::from_str(value, false).map_err(|_| ValueError("expected auto, always, or never"))
    });
    validator.require(key, input, color)
}

pub(crate) fn path(
    validator: &mut Validator<'_>,
    key: &str,
    input: &Input<'_>,
    dirs: &PlatformDirs,
) -> Option<Option<AbsolutePath>> {
    let path = text(input.get_ref()).and_then(|value| match value {
        "" => Ok(None),
        value => AbsolutePath::new(dirs.expand_home(Path::new(value))?).map(Some),
    });
    validator.require(key, input, path)
}

pub(crate) fn list<T: FromStr<Err = ValueError>>(
    validator: &mut Validator<'_>,
    key: &str,
    input: &Input<'_>,
) -> Option<Vec<T>> {
    validator.list(key, input, Validator::parse)
}

pub(crate) fn bootstrap_list(
    validator: &mut Validator<'_>,
    key: &str,
    input: &Input<'_>,
) -> Option<Vec<BootstrapNode>> {
    validator.list(key, input, bootstrap_node)
}

pub(crate) fn default_error(error: ValueError) -> CliError {
    CliError::new(
        crate::error::ErrorKind::InvalidConfig,
        format!("Configuration defaults: {error}"),
    )
}

fn bootstrap_node(
    validator: &mut Validator<'_>,
    key: &str,
    input: &Input<'_>,
) -> Option<BootstrapNode> {
    let table = input
        .get_ref()
        .as_table()
        .ok_or(ValueError("expected a bootstrap node table"));
    let table = validator.require(key, input, table)?;
    let mut node_id = None;
    let mut relay_url = None;
    let keys = ["node_id", "relay_url"].map(|field| format!("{key}.{field}"));
    for (name, value) in table {
        match name.get_ref().as_ref() {
            "node_id" => node_id = Some(validator.parse(&keys[0], value)),
            "relay_url" => relay_url = Some(validator.parse(&keys[1], value)),
            field => validator.unknown(
                &format!("{key}.{field}"),
                name.span(),
                keys.iter().map(String::as_str),
            ),
        }
    }
    for (missing, name) in [node_id.is_none(), relay_url.is_none()]
        .into_iter()
        .zip(keys)
    {
        if missing {
            validator.report(&name, Some(input.span()), "missing required key");
        }
    }
    Some(BootstrapNode {
        node_id: node_id.flatten()?,
        relay_url: relay_url.flatten()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Platform;

    fn validate(text: &str) -> Result<Config, CliError> {
        let home = Path::new("/home/release");
        let dirs = PlatformDirs::new(Platform::Linux, home, None, None);
        validate_file(&home.join("config.toml"), text, &dirs)
    }

    #[test]
    fn validates_the_daemon_table() {
        let config =
            validate("[daemon]\nautostart = false\nsocket_path = '~/release.sock'").unwrap();
        assert!(!config.daemon.autostart.value);
        assert_eq!(
            config.daemon.socket_path.value.unwrap().as_path(),
            Path::new("/home/release/release.sock")
        );
        assert!(
            validate("[daemon]\nautostrat = true")
                .unwrap_err()
                .to_string()
                .contains("`daemon.autostart`")
        );
    }

    #[test]
    fn validates_the_optional_fixed_port() {
        assert_eq!(validate("").unwrap().port.value, None);
        assert_eq!(validate("port = 44123").unwrap().port.value, Some(44123));
        for text in ["port = 0", "port = 65536", "port = '44123'"] {
            assert!(validate(text).unwrap_err().to_string().contains("port:"));
        }
    }

    #[test]
    fn validates_each_key_and_every_bootstrap_field() {
        validate(
            r#"
network = "release-team"
relay_urls = ["https://relay.release.example"]
mdns = false
credentials_file = "~/keys/device.cbor"
trusted_devices = ["00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"]
data_dir = "/srv/pepino"
[[bootstrap_nodes]]
node_id = "ffeeccbb445566778899aabbccddeeff00112233445566778899aabbccddee12"
relay_url = "https://bootstrap.release.example"
[ui]
color = "never"
date_format = "%d %b %Y"
"#,
        )
        .unwrap();

        let error = validate(
            r#"network = ""
relay_urls = ["http://relay.release.example", false]
mdns = "false"
credentials_file = "keys/device.cbor"
trusted_devices = ["123"]
data_dir = "./local"
[[bootstrap_nodes]]
node_id = "unknown-device"
relay_url = "bogus"
[[bootstrap_nodes]]
node_id = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
[ui]
color = "rainbow"
date_format = "%Q"
"#,
        )
        .unwrap_err()
        .to_string();
        for key in [
            "network",
            "relay_urls[0]",
            "relay_urls[1]",
            "mdns",
            "credentials_file",
            "trusted_devices[0]",
            "data_dir",
            "bootstrap_nodes[0].node_id",
            "bootstrap_nodes[0].relay_url",
            "bootstrap_nodes[1].relay_url",
            "ui.color",
            "ui.date_format",
        ] {
            assert!(error.contains(&format!(": {key}: ")), "{error}");
        }
        assert_eq!(error.lines().count(), 12, "{error}");
        assert!(
            error.contains("/home/release/config.toml:1:11: network:"),
            "{error}"
        );
    }

    #[test]
    fn recovers_syntax_errors_and_suggests_misspelled_keys() {
        let error = validate("network = ?\nmdns = ?\nrelay_urls = ['https://relay.example', ?]\nnetwork = 'duplicate'\n[ui]\ndate_fromat = '%Y'\ncolor = ?\n")
            .unwrap_err()
            .to_string();
        assert!(error.contains("config.toml:1:11: network:"), "{error}");
        assert!(error.contains("config.toml:2:8: mdns:"), "{error}");
        assert!(
            error.contains(": relay_urls[1]: string values must be quoted"),
            "{error}"
        );
        assert!(
            error.contains("config.toml:4:1: network: duplicate key"),
            "{error}"
        );
        assert!(error.contains("config.toml:7:9: ui.color:"), "{error}");
        assert_eq!(error.lines().count(), 6, "{error}");
        assert!(
            error.contains("ui.date_fromat: unknown key; did you mean `ui.date_format`?"),
            "{error}"
        );
    }

    #[test]
    fn derives_generated_credentials_after_merging_paths() {
        let home = Path::new("/home/release");
        let dirs = PlatformDirs::new(Platform::Linux, home, None, None);
        let mut draft = Draft::new(&dirs);
        draft
            .apply_file(
                &home.join("config.toml"),
                "data_dir = '/srv/pepino'\ncredentials_file = ''",
                &Source::Env,
            )
            .unwrap();
        let config = draft.finish().unwrap();
        assert_eq!(
            config.credentials_file.value.as_path(),
            Path::new("/srv/pepino/credentials.cbor")
        );
        assert_eq!(config.credentials_file.source, Source::Env);
    }
}
