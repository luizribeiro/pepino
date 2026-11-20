use std::path::Path;

use clap::ValueEnum;
use toml::de::DeValue;

use super::diagnostics::invalid_config;
use super::document::{Input, Validator, parse_value, text};
use super::{AbsolutePath, BootstrapNode, Config, PlatformDirs, Setting, Source, ValueError};
use crate::error::CliError;
use crate::terminal::Color;

pub(crate) const ROOT_KEYS: &[&str] = &[
    "network",
    "relay_urls",
    "mdns",
    "credentials_file",
    "trusted_devices",
    "bootstrap_nodes",
    "data_dir",
    "ui",
];
pub(crate) const UI_KEYS: &[&str] = &["ui.color", "ui.date_format"];

pub fn validate_file(path: &Path, text: &str, dirs: &PlatformDirs) -> Result<Config, CliError> {
    let mut draft = Draft::new(dirs);
    draft.apply_file(
        path,
        text,
        &Source::ExplicitFile {
            path: path.to_owned(),
        },
    )?;
    Ok(draft.finish())
}

pub(crate) struct Draft<'a> {
    pub dirs: &'a PlatformDirs,
    pub config: Config,
    use_generated_credentials: bool,
}

impl<'a> Draft<'a> {
    pub fn new(dirs: &'a PlatformDirs) -> Self {
        Self {
            dirs,
            config: Config::defaults(dirs),
            use_generated_credentials: true,
        }
    }

    pub fn finish(mut self) -> Config {
        if self.use_generated_credentials {
            self.config.credentials_file.value =
                self.config.data_dir.value.join("credentials.cbor");
        }
        self.config
    }

    pub fn apply_file(&mut self, path: &Path, text: &str, source: &Source) -> Result<(), CliError> {
        let mut validator = Validator::new(Some((path, text)));
        for (key, value) in validator.document(text) {
            match key.get_ref().as_ref() {
                "ui" => {
                    let table = value
                        .get_ref()
                        .as_table()
                        .ok_or(ValueError("expected a table"));
                    if let Some(table) = validator.require("ui", &value, table) {
                        for (key, value) in table {
                            let name = format!("ui.{}", key.get_ref());
                            if UI_KEYS.contains(&name.as_str()) {
                                self.apply(&name, value, source, &mut validator);
                            } else {
                                validator.unknown(&name, key.span(), UI_KEYS);
                            }
                        }
                    }
                }
                name if ROOT_KEYS.contains(&name) => {
                    self.apply(name, &value, source, &mut validator)
                }
                name => validator.unknown(name, key.span(), ROOT_KEYS),
            }
        }
        invalid_config(validator.problems)
    }

    pub fn apply(
        &mut self,
        key: &str,
        input: &Input<'_>,
        source: &Source,
        validator: &mut Validator<'_>,
    ) {
        let value = input.get_ref();
        macro_rules! assign {
            ($field:expr, $result:expr) => {
                if let Some(value) = validator.require(key, input, $result) {
                    $field = Setting {
                        value,
                        source: source.clone(),
                    };
                }
            };
        }
        match key {
            "network" => assign!(self.config.network, parse_value(value)),
            "mdns" => assign!(
                self.config.mdns,
                value.as_bool().ok_or(ValueError("expected a boolean"))
            ),
            "ui.color" => assign!(
                self.config.ui.color,
                text(value).and_then(|value| {
                    Color::from_str(value, false)
                        .map_err(|_| ValueError("expected auto, always, or never"))
                })
            ),
            "ui.date_format" => assign!(self.config.ui.date_format, parse_value(value)),
            "data_dir" => {
                let path = self
                    .configured_path(value)
                    .map(|path| path.unwrap_or_else(|| self.dirs.data_dir.clone()));
                assign!(self.config.data_dir, path);
            }
            "credentials_file" => {
                if let Some(path) = validator.require(key, input, self.configured_path(value)) {
                    self.use_generated_credentials = path.is_none();
                    self.config.credentials_file = Setting {
                        value: path
                            .unwrap_or_else(|| self.config.data_dir.value.join("credentials.cbor")),
                        source: source.clone(),
                    };
                }
            }
            "relay_urls" => {
                if let Some(values) = validator.list(key, input, Validator::parse) {
                    self.config.relay_urls = Setting {
                        value: values,
                        source: source.clone(),
                    };
                }
            }
            "trusted_devices" => {
                if let Some(values) = validator.list(key, input, Validator::parse) {
                    self.config.trusted_devices = Setting {
                        value: values,
                        source: source.clone(),
                    };
                }
            }
            "bootstrap_nodes" => {
                if let Some(values) = validator.list(key, input, bootstrap_node) {
                    self.config.bootstrap_nodes = Setting {
                        value: values,
                        source: source.clone(),
                    };
                }
            }
            _ => unreachable!("keys are checked before applying them"),
        }
    }

    fn configured_path(&self, input: &DeValue<'_>) -> Result<Option<AbsolutePath>, ValueError> {
        match text(input)? {
            "" => Ok(None),
            value => AbsolutePath::expand(Path::new(value), self.dirs.home.as_path()).map(Some),
        }
    }
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
                &[&keys[0], &keys[1]],
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
        let dirs = PlatformDirs::new(Platform::Linux, home, None, None).unwrap();
        validate_file(&home.join("config.toml"), text, &dirs)
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
        let dirs = PlatformDirs::new(Platform::Linux, home, None, None).unwrap();
        let mut draft = Draft::new(&dirs);
        draft
            .apply_file(
                &home.join("config.toml"),
                "data_dir = '/srv/pepino'\ncredentials_file = ''",
                &Source::Env,
            )
            .unwrap();
        let config = draft.finish();
        assert_eq!(
            config.credentials_file.value.as_path(),
            Path::new("/srv/pepino/credentials.cbor")
        );
        assert_eq!(config.credentials_file.source, Source::Env);
    }
}
