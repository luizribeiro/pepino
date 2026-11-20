use std::ffi::OsStr;

use toml::Spanned;
use toml::de::{DeTable, DeValue};

use super::diagnostics::invalid_config;
use super::document::{Input, Validator};
use super::model::{Draft, ROOT_KEYS, UI_KEYS};
use super::{Env, Source, ValueError};
use crate::error::CliError;

pub fn env_value<'a>(env: &'a Env, name: &str) -> Result<Option<&'a str>, ValueError> {
    env.get(OsStr::new(name))
        .filter(|value| !value.is_empty())
        .map(|value| value.to_str().ok_or(ValueError("expected a UTF-8 value")))
        .transpose()
}

pub(crate) fn apply(draft: &mut Draft<'_>, env: &Env) -> Result<(), CliError> {
    let mut problems = Vec::new();
    for entry in ROOT_KEYS.iter().chain(UI_KEYS) {
        let variable = entry.environment.variable;
        let key = entry.environment.key;
        let raw = match env_value(env, variable) {
            Ok(Some(raw)) => raw,
            Ok(None) => continue,
            Err(error) => {
                problems.push(format!("{variable}: {key}: {error}"));
                continue;
            }
        };
        let mut validator = Validator::new(None);
        (entry.apply)(
            draft,
            &wrapped((entry.env_input)(raw)),
            &Source::Env,
            &mut validator,
        );
        problems.extend(
            validator
                .problems
                .into_iter()
                .map(|problem| format!("{variable}: {problem}")),
        );
    }
    invalid_config(problems)
}

pub(crate) fn scalar(raw: &str) -> DeValue<'_> {
    DeValue::String(raw.into())
}

pub(crate) use scalar as color;
pub(crate) use scalar as path;

pub(crate) fn boolean(raw: &str) -> DeValue<'_> {
    match raw {
        "1" | "true" => DeValue::Boolean(true),
        "0" | "false" => DeValue::Boolean(false),
        _ => scalar(raw),
    }
}

pub(crate) fn list(raw: &str) -> DeValue<'_> {
    comma_list(raw, scalar)
}

pub(crate) fn bootstrap_list(raw: &str) -> DeValue<'_> {
    comma_list(raw, bootstrap)
}

fn comma_list<'a>(raw: &'a str, item: fn(&'a str) -> DeValue<'a>) -> DeValue<'a> {
    DeValue::Array(
        raw.split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(|value| wrapped(item(value)))
            .collect(),
    )
}

fn bootstrap(value: &str) -> DeValue<'_> {
    let Some((id, relay)) = value.split_once('=') else {
        return DeValue::String(value.into());
    };
    let table = DeTable::from_iter([("node_id", id.trim()), ("relay_url", relay.trim())].map(
        |(key, value)| {
            (
                Spanned::new(0..0, key.into()),
                wrapped(DeValue::String(value.into())),
            )
        },
    ));
    DeValue::Table(table)
}

fn wrapped(value: DeValue<'_>) -> Input<'_> {
    Spanned::new(0..0, value)
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::Path;

    use super::*;
    use crate::config::{Flags, Platform, PlatformDirs, load};
    use crate::terminal::Color;

    #[test]
    fn reads_each_variable_and_resolves_precedence() {
        let dirs = PlatformDirs::new(Platform::Linux, Path::new("/home/release"), None, None);
        let id = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let bootstrap = format!(" , {id} = https://bootstrap.release.example , ");
        let env = Env::from(
            [
                ("PPN_NETWORK", "env-release"),
                (
                    "PPN_RELAY_URLS",
                    "https://use.release.example,, https://eu.release.example,",
                ),
                ("PPN_MDNS", "0"),
                ("PPN_CREDENTIALS_FILE", "~/keys/device.cbor"),
                ("PPN_TRUSTED_DEVICES", id),
                ("PPN_BOOTSTRAP_NODES", &bootstrap),
                ("PPN_DATA_DIR", "/srv/pepino"),
                ("PPN_COLOR", "always"),
                ("PPN_DATE_FORMAT", "%d %b %Y"),
            ]
            .map(|(name, value)| (name.into(), value.into())),
        );
        let read = |path: &Path| {
            if path == dirs.user_config().unwrap().as_path() {
                Ok(format!(
                    "network = 'user-release'\ntrusted_devices = ['{id}']\n[ui]\ncolor = 'auto'"
                ))
            } else {
                Err(io::ErrorKind::NotFound.into())
            }
        };
        let loaded = load(&Flags::default(), &env, read, &dirs).unwrap();
        let values = loaded.config.values();
        assert!(
            values
                .values()
                .all(|value| value["source"] == "env" && value.get("path").is_none())
        );
        assert_eq!(values["network"]["value"], "env-release");
        assert_eq!(
            values["relay_urls"]["value"][1],
            "https://eu.release.example"
        );
        assert_eq!(values["mdns"]["value"], false);
        assert_eq!(
            values["credentials_file"]["value"],
            "/home/release/keys/device.cbor"
        );
        assert_eq!(values["trusted_devices"]["value"], serde_json::json!([id]));
        assert_eq!(values["bootstrap_nodes"]["value"][0]["node_id"], id);
        assert_eq!(
            values["bootstrap_nodes"]["value"][0]["relay_url"],
            "https://bootstrap.release.example"
        );
        assert_eq!(values["data_dir"]["value"], "/srv/pepino");
        assert_eq!(values["ui.color"]["value"], "always");
        assert_eq!(values["ui.date_format"]["value"], "%d %b %Y");

        let flags = Flags {
            color: Some(Color::Never),
            ..Flags::default()
        };
        let loaded = load(&flags, &env, read, &dirs).unwrap();
        assert_eq!(loaded.config.ui.color.value, Color::Never);
        assert_eq!(loaded.config.ui.color.source, Source::Flag);
    }

    #[test]
    fn names_each_invalid_environment_variable() {
        let dirs = PlatformDirs::new(Platform::Linux, Path::new("/home/release"), None, None);
        let env = Env::from([
            ("PPN_MDNS".into(), "sometimes".into()),
            (
                "PPN_BOOTSTRAP_NODES".into(),
                "no-node-at-relay,invalid=http://relay.example".into(),
            ),
        ]);
        let error = apply(&mut Draft::new(&dirs), &env).unwrap_err().to_string();
        for key in [
            "PPN_MDNS: mdns",
            "PPN_BOOTSTRAP_NODES: bootstrap_nodes[0]",
            "bootstrap_nodes[1].node_id",
            "bootstrap_nodes[1].relay_url",
        ] {
            assert!(error.contains(key), "{error}");
        }
    }
}
