use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::{Value, json};

use super::document::{Input, Validator};
use super::path::serialize_path;
use super::{
    AbsolutePath, CONFIG_ENV, DateFormat, Network, NodeId, PlatformDirs, RelayUrl, ValueError,
};
use super::{env, validate};
use crate::terminal::Color;
use toml::de::DeValue;

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
    fn pending() -> Setting<Option<T>> {
        Setting {
            value: None,
            source: Source::Default,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BootstrapNode {
    pub node_id: NodeId,
    pub relay_url: RelayUrl,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct EnvironmentVariable {
    pub variable: &'static str,
    pub key: &'static str,
    pub value: &'static str,
}

pub(crate) struct Key {
    pub environment: EnvironmentVariable,
    pub env_input: for<'a> fn(&'a str) -> DeValue<'a>,
    pub apply: fn(&mut Draft<'_>, &Input<'_>, &Source, &mut Validator<'_>),
}

pub fn environment_variables() -> impl Iterator<Item = EnvironmentVariable> {
    std::iter::once(EnvironmentVariable {
        variable: CONFIG_ENV,
        key: "File selection",
        value: "Path to one required TOML file. --config takes precedence.",
    })
    .chain(keys().map(|key| key.environment))
}

pub(crate) fn keys() -> impl Iterator<Item = &'static Key> {
    ROOT_KEYS
        .iter()
        .chain(TABLES.iter().flat_map(|(_, keys)| *keys))
}

impl<T> Setting<Option<T>> {
    fn resolve(
        self,
        default: impl FnOnce() -> Result<T, ValueError>,
    ) -> Result<Setting<T>, ValueError> {
        Ok(Setting {
            value: self.value.map_or_else(default, Ok)?,
            source: self.source,
        })
    }
}

macro_rules! parse_setting {
    (path, $draft:ident, $validator:ident, $key:expr, $input:ident) => {
        validate::path($validator, $key, $input, $draft.dirs)
    };
    (optional_path, $draft:ident, $validator:ident, $key:expr, $input:ident) => {
        validate::path($validator, $key, $input, $draft.dirs).map(Some)
    };
    ($parser:ident, $draft:ident, $validator:ident, $key:expr, $input:ident) => {
        validate::$parser($validator, $key, $input).map(Some)
    };
}

macro_rules! key {
    ($name:expr, $variable:literal, $description:literal, $parser:ident, $($field:ident).+) => {
        Key {
            environment: EnvironmentVariable { variable: $variable, key: $name, value: $description },
            env_input: env::$parser,
            apply: |draft, input, source, validator| {
                if let Some(value) = parse_setting!($parser, draft, validator, $name, input) {
                    draft.$($field).+ = Setting { value, source: source.clone() };
                }
            },
        }
    };
}

macro_rules! settings {
    ($dirs:ident;
     root { $($field:ident: $ty:ty = $parser:ident($variable:literal, $description:literal, $default:expr);)* }
     $($table:ident ($config:ident, $draft:ident, $keys:ident) {
         $($field_name:ident: $value:ty = $parse:ident($variable_name:literal, $help:literal, $fallback:expr);)*
     })*
    ) => {
        #[derive(Clone, Debug)]
        pub struct Config {
            $(pub $field: Setting<$ty>,)*
            $(pub $table: $config,)*
        }

        $(
            #[derive(Clone, Debug)]
            pub struct $config {
                $(pub $field_name: Setting<$value>,)*
            }
        )*

        pub(crate) struct Draft<'a> {
            pub dirs: &'a PlatformDirs,
            $($field: Setting<Option<$ty>>,)*
            $(pub $table: $draft,)*
        }

        $(
            pub(crate) struct $draft {
                $(pub $field_name: Setting<Option<$value>>,)*
            }
        )*

        impl<'a> Draft<'a> {
            pub fn new(dirs: &'a PlatformDirs) -> Self {
                Self {
                    dirs,
                    $($field: Setting::<$ty>::pending(),)*
                    $($table: $draft { $($field_name: Setting::<$value>::pending(),)* },)*
                }
            }

            pub fn finish(self) -> Result<Config, ValueError> {
                let $dirs = self.dirs;
                $(let $field = self.$field.resolve(|| $default)?;)*
                Ok(Config {
                    $($field,)*
                    $($table: $config { $($field_name: self.$table.$field_name.resolve(|| $fallback)?,)* },)*
                })
            }
        }

        impl Config {
            pub fn defaults(dirs: &PlatformDirs) -> Result<Self, ValueError> {
                Draft::new(dirs).finish()
            }

            pub fn values(&self) -> BTreeMap<&'static str, Value> {
                BTreeMap::from([
                    $((stringify!($field), json!(self.$field)),)*
                    $($((concat!(stringify!($table), ".", stringify!($field_name)), json!(self.$table.$field_name)),)*)*
                ])
            }
        }

        pub(crate) const ROOT_KEYS: &[Key] = &[
            $(key!(stringify!($field), $variable, $description, $parser, $field),)*
        ];
        $(
            const $keys: &[Key] = &[
                $(key!(concat!(stringify!($table), ".", stringify!($field_name)), $variable_name, $help, $parse, $table.$field_name),)*
            ];
        )*
        pub(crate) const TABLES: &[(&str, &[Key])] = &[$((stringify!($table), $keys),)*];
    };
}

settings! {
    dirs;
    root {
        network: Network = scalar("PPN_NETWORK", "Non-empty network name.", "pepino".parse());
        relay_urls: Vec<RelayUrl> = list("PPN_RELAY_URLS", "Comma-separated HTTPS URLs.",
            DEFAULT_RELAY_URLS.iter().map(|url| url.parse()).collect());
        port: Option<u16> = port("PPN_PORT", "UDP port from 1 to 65535.", Ok(None));
        mdns: bool = boolean("PPN_MDNS", "true, false, 1, or 0.", Ok(true));
        data_dir: AbsolutePath = path("PPN_DATA_DIR", "Absolute path, ~, or a path beginning with ~/.", dirs.data_dir());
        credentials_file: AbsolutePath = path("PPN_CREDENTIALS_FILE", "Absolute path, ~, or a path beginning with ~/.",
            Ok(data_dir.value.join("credentials.cbor")));
        trusted_devices: Vec<NodeId> = list("PPN_TRUSTED_DEVICES", "Comma-separated 64-digit hex node IDs.", Ok(Vec::new()));
        bootstrap_nodes: Vec<BootstrapNode> = bootstrap_list("PPN_BOOTSTRAP_NODES",
            "Comma-separated node_id=https://relay.example pairs with 64-digit hex node IDs.", Ok(Vec::new()));
    }
    daemon (DaemonConfig, DraftDaemon, DAEMON_KEYS) {
        autostart: bool = boolean("PPN_DAEMON_AUTOSTART", "true, false, 1, or 0.", Ok(true));
        socket_path: Option<AbsolutePath> = optional_path("PPN_DAEMON_SOCKET_PATH", "Absolute path or a path beginning with ~/.", Ok(None));
    }
    ui (UiConfig, DraftUi, UI_KEYS) {
        color: Color = color("PPN_COLOR", "auto, always, or never. --color takes precedence.", Ok(Color::Auto));
        date_format: DateFormat = scalar("PPN_DATE_FORMAT", "Strftime format.", "%Y-%m-%d".parse());
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::config::Platform;

    #[test]
    fn defaults_cover_the_schema_with_default_sources() {
        let dirs = PlatformDirs::new(Platform::Linux, Path::new("/home/release"), None, None);
        let values = Config::defaults(&dirs).unwrap().values();
        assert_eq!(
            values
                .keys()
                .copied()
                .collect::<std::collections::BTreeSet<_>>(),
            environment_variables()
                .skip(1)
                .map(|variable| variable.key)
                .collect()
        );
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
        assert_eq!(values["daemon.autostart"]["value"], true);
        assert!(values["daemon.socket_path"]["value"].is_null());
        assert_eq!(values["ui.color"]["value"], "auto");
        assert_eq!(values["ui.date_format"]["value"], "%Y-%m-%d");
    }
}
