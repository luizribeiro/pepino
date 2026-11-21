use std::time::Duration;

use pepino_app::OpenOptions;
use pepino_core::DeviceId;
use ppn::{
    daemon::{address::socket_path, client::Client, protocol::Operation},
    error::{CliError, ErrorKind},
    help_tree::example_help,
    output::{CommandResult, Render},
    terminal::{Context, Terminal},
};
use serde_json::{Map, Value, json};

#[derive(clap::Args)]
#[command(about = "Manage this relay's node credentials.", after_help = example_help(&["ppn relay init", "ppn relay show --json"]))]
pub struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Initialise relay credentials in an unused data directory.
    #[command(after_help = example_help(&["ppn relay init --json"]))]
    Init,
    /// Show this relay's node ID and connection address.
    #[command(after_help = example_help(&["ppn relay show --json"]))]
    Show,
}

struct RelayInfo {
    node_id: DeviceId,
    relay_url: Option<String>,
}

impl RelayInfo {
    fn from_config(config: &ppn::config::Config, node_id: DeviceId) -> Self {
        Self {
            node_id,
            relay_url: config
                .relay_urls
                .value
                .first()
                .map(|url| url.as_url().as_str().to_owned()),
        }
    }

    fn address(&self) -> String {
        match &self.relay_url {
            Some(url) => format!("{}@{url}", self.node_id),
            None => self.node_id.to_string(),
        }
    }
}

impl Render for RelayInfo {
    fn human(&self, _: &Terminal) -> String {
        format!("Relay node {}\nAddress {}", self.node_id, self.address())
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), json!("relay")),
            ("node_id".into(), json!(self.node_id)),
            ("address".into(), json!(self.address())),
            ("relay_url".into(), json!(self.relay_url)),
        ])
    }
}

pub fn run(context: &Context<'_>, args: Args) -> Result<CommandResult, CliError> {
    let config = &context.config()?.config;
    let directory = config.data_dir.value.as_path();
    let path = config.credentials_file.value.as_path();
    let keys = match args.action {
        Action::Init => {
            let mut options = OpenOptions::default();
            if let Some(timeout) = context.timeout {
                options.lock_timeout = timeout;
            }
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| CliError::new(ErrorKind::Failure, error.to_string()))?
                .block_on(pepino_app::init_relay(directory, path, options))?
        }
        Action::Show => pepino_app::relay_credentials(directory, path)?,
    };
    Ok(CommandResult::document(RelayInfo::from_config(
        config,
        keys.device_id(),
    )))
}

struct Status {
    info: RelayInfo,
    carrying: Option<pepino_node::RelayStatus>,
}

impl Render for Status {
    fn human(&self, terminal: &Terminal) -> String {
        let mut text = self.info.human(terminal);
        if let Some(status) = &self.carrying {
            use std::fmt::Write;
            let _ = write!(
                text,
                "\nCarrying {} of {} boards · {} bytes stored · {} rejected instructions\nPeers seen {}",
                status.boards_by_device.values().sum::<usize>(),
                status.max_boards,
                status.bytes_stored,
                status.rejected_instructions,
                status.peers.len()
            );
            for peer in &status.peers {
                let _ = write!(text, "\n  Peer {peer}");
            }
            for (node, count) in &status.boards_by_device {
                let _ = write!(text, "\n{node}  {count} boards");
            }
            if status.removal_pending_restart != 0 {
                let _ = write!(
                    text,
                    "\n{} boards with removal pending restart",
                    status.removal_pending_restart
                );
            }
        } else {
            text.push_str("\nOffline (daemon not running)");
        }
        text
    }

    fn json_fields(self) -> Map<String, Value> {
        let mut fields = self.info.json_fields();
        fields.insert("role".into(), json!("relay"));
        fields.insert("online".into(), json!(self.carrying.is_some()));
        fields.insert("relay".into(), json!(self.carrying));
        fields
    }
}

pub fn status(context: &Context<'_>) -> Result<CommandResult, CliError> {
    let config = &context.config()?.config;
    let keys = pepino_app::relay_credentials(
        config.data_dir.value.as_path(),
        config.credentials_file.value.as_path(),
    )?;
    let client = Client {
        path: socket_path(config),
    };
    let carrying = match client.call_with(Operation::Status, Duration::from_secs(2)) {
        Ok(ppn::daemon::protocol::Outcome::Status { status }) => status.relay,
        Ok(_) => {
            return Err(CliError::new(
                ErrorKind::Failure,
                "Could not read the relay status.",
            ));
        }
        Err(error) if ppn::daemon::client::unavailable(&error) => None,
        Err(_) => {
            return Err(CliError::new(
                ErrorKind::DaemonUnreachable,
                "Could not reach the relay daemon.",
            ));
        }
    };
    Ok(CommandResult::document(Status {
        info: RelayInfo::from_config(config, keys.device_id()),
        carrying,
    }))
}
