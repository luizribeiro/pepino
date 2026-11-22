use std::time::Duration;

use pepino_app::OpenOptions;
use pepino_core::DeviceId;
use pepino_node::RelayDrop;
use ppn::{
    config::Config,
    daemon::{
        address::socket_path,
        client::Client,
        protocol::{Operation, Outcome},
    },
    error::{CliError, ErrorKind},
    help_tree::example_help,
    output::{CommandResult, Render},
    terminal::{Context, Terminal},
};
use serde_json::{Map, Value, json};

#[derive(clap::Args)]
#[command(about = "Manage this relay's node credentials and carried boards.", after_help = example_help(&["ppn relay init", "ppn relay show --json", "ppn relay boards"]))]
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
    /// List the boards a running relay carries, by requesting device.
    #[command(after_help = example_help(&["ppn relay boards --json"]))]
    Boards,
    /// Delete a running relay's stored copy of one board, or of every board a device requested.
    #[command(after_help = example_help(&["ppn relay drop 3f9a12c4d5e6 --yes", "ppn relay drop --from NODE_ID --yes"]))]
    Drop {
        /// Board handle from ppn relay boards.
        #[arg(
            value_name = "HANDLE",
            required_unless_present = "from",
            conflicts_with = "from"
        )]
        handle: Option<String>,
        /// Drop every board requested by this device node ID.
        #[arg(long, value_name = "NODE-ID")]
        from: Option<String>,
    },
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
        Action::Boards => return boards(context),
        Action::Drop { handle, from } => return drop_boards(context, handle, from),
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
                "\n{} · {} bytes stored · {} rejected instructions\nPeers seen {}",
                carrying(status),
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

// A board appears once for every device that requested it.
fn carried_boards(status: &pepino_node::RelayStatus) -> usize {
    let handles: std::collections::BTreeSet<_> =
        status.boards.iter().map(|board| &board.handle).collect();
    handles.len()
}

fn carrying(status: &pepino_node::RelayStatus) -> String {
    format!(
        "Carrying {} of {} boards",
        carried_boards(status),
        status.max_boards
    )
}

fn relay_keys(config: &Config) -> Result<pepino_node::DeviceCredentials, CliError> {
    Ok(pepino_app::relay_credentials(
        config.data_dir.value.as_path(),
        config.credentials_file.value.as_path(),
    )?)
}

/// Returns `None` when no daemon is running.
fn call(config: &Config, operation: Operation) -> Result<Option<Outcome>, CliError> {
    let client = Client {
        path: socket_path(config),
    };
    match client.call_with(operation, Duration::from_secs(2)) {
        Ok(outcome) => Ok(Some(outcome)),
        Err(error) if ppn::daemon::client::unavailable(&error) => Ok(None),
        Err(_) => Err(CliError::new(
            ErrorKind::DaemonUnreachable,
            "Could not reach the relay daemon.",
        )),
    }
}

fn running_status(config: &Config) -> Result<Option<pepino_node::RelayStatus>, CliError> {
    match call(config, Operation::Status)? {
        Some(Outcome::Status { status }) => Ok(status.relay),
        Some(_) => Err(CliError::new(
            ErrorKind::Failure,
            "Could not read the relay status.",
        )),
        None => Ok(None),
    }
}

fn not_running() -> CliError {
    CliError::new(
        ErrorKind::DaemonUnreachable,
        "The relay daemon is not running.",
    )
    .with_hint("Run ppn daemon start.")
}

pub fn status(context: &Context<'_>) -> Result<CommandResult, CliError> {
    let config = &context.config()?.config;
    let keys = relay_keys(config)?;
    Ok(CommandResult::document(Status {
        info: RelayInfo::from_config(config, keys.device_id()),
        carrying: running_status(config)?,
    }))
}

struct Boards(pepino_node::RelayStatus);

impl Boards {
    fn requested_by(&self, device: DeviceId) -> impl Iterator<Item = &pepino_node::CarriedBoard> {
        self.0
            .boards
            .iter()
            .filter(move |board| board.requested_by == device)
    }
}

impl Render for Boards {
    fn human(&self, _: &Terminal) -> String {
        use std::fmt::Write;
        let mut text = carrying(&self.0);
        for (&node, count) in &self.0.boards_by_device {
            let _ = write!(text, "\n{node}  {count} boards");
            for board in self.requested_by(node) {
                let pending = if board.removal_pending {
                    " (removal pending restart)"
                } else {
                    ""
                };
                let _ = write!(text, "\n  {}{pending}", board.handle);
            }
        }
        text
    }

    fn json_fields(self) -> Map<String, Value> {
        let devices: Vec<_> = self
            .0
            .boards_by_device
            .keys()
            .map(|&node| {
                let boards: Vec<_> = self
                    .requested_by(node)
                    .map(|board| json!({"handle": board.handle, "removal_pending": board.removal_pending}))
                    .collect();
                json!({"node_id": node, "boards": boards})
            })
            .collect();
        Map::from_iter([
            ("kind".into(), json!("relay_boards")),
            ("count".into(), json!(carried_boards(&self.0))),
            ("max_boards".into(), json!(self.0.max_boards)),
            ("devices".into(), json!(devices)),
        ])
    }
}

fn boards(context: &Context<'_>) -> Result<CommandResult, CliError> {
    let config = &context.config()?.config;
    relay_keys(config)?;
    let status = running_status(config)?.ok_or_else(not_running)?;
    Ok(CommandResult::document(Boards(status)))
}

struct Dropped(pepino_node::DroppedBoards);

impl Dropped {
    fn consequence(&self) -> String {
        match self.0.purged {
            0 => "Other devices still request them, so the relay keeps its copies.".into(),
            purged => format!("The relay restarts its node and deletes {purged} stored copies."),
        }
    }
}

impl Render for Dropped {
    fn human(&self, _: &Terminal) -> String {
        format!("Dropped {} boards. {}", self.0.boards, self.consequence())
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), json!("relay_drop")),
            ("dropped".into(), json!(self.0.boards)),
            ("purged".into(), json!(self.0.purged)),
        ])
    }
}

fn drop_boards(
    context: &Context<'_>,
    handle: Option<String>,
    from: Option<String>,
) -> Result<CommandResult, CliError> {
    let config = &context.config()?.config;
    relay_keys(config)?;
    let boards = match (handle, from) {
        (Some(handle), _) => RelayDrop::Handle(handle),
        (None, Some(node)) => RelayDrop::From(node.parse().map_err(|_| {
            CliError::new(
                ErrorKind::InvalidInput,
                "Expected a 64-digit device node ID.",
            )
        })?),
        (None, None) => {
            return Err(CliError::new(
                ErrorKind::Usage,
                "Give a board handle or --from NODE-ID.",
            ));
        }
    };
    if !context.confirm("Delete this relay's stored copy of the selected boards?")? {
        return Err(CliError::new(
            ErrorKind::ConfirmationRequired,
            "Relay drop was not confirmed.",
        ));
    }
    match call(config, Operation::RelayDrop { boards })?.ok_or_else(not_running)? {
        Outcome::RelayDropped { dropped } => Ok(CommandResult::document(Dropped(dropped))),
        Outcome::Error { code, message } if code == "not_found" => {
            Err(CliError::new(ErrorKind::NotFound, message)
                .with_hint("Run ppn relay boards to list the handles."))
        }
        Outcome::Error { message, .. } => Err(CliError::new(ErrorKind::Failure, message)),
        _ => Err(CliError::new(
            ErrorKind::Failure,
            "Could not drop the boards.",
        )),
    }
}

#[cfg(test)]
#[test]
fn a_board_requested_by_two_devices_is_listed_under_each_and_counted_once() {
    let [laptop, phone] = [[17; 32], [27; 32]].map(DeviceId::from_bytes);
    let board = |handle: &str, requested_by, removal_pending| pepino_node::CarriedBoard {
        handle: handle.into(),
        requested_by,
        removal_pending,
    };
    let status = pepino_node::RelayStatus {
        boards_by_device: [(laptop, 1), (phone, 1)].into(),
        max_boards: 8,
        boards: vec![
            board("3f9a12c4d5e6", laptop, false),
            board("3f9a12c4d5e6", phone, false),
            board("a1b2c3d4e5f6", laptop, true),
        ],
        ..Default::default()
    };
    assert_eq!(carrying(&status), "Carrying 2 of 8 boards");
    let fields = Boards(status).json_fields();
    assert_eq!(fields["count"], 2);
    assert_eq!(
        fields["devices"],
        json!([
            {"node_id": laptop, "boards": [
                {"handle": "3f9a12c4d5e6", "removal_pending": false},
                {"handle": "a1b2c3d4e5f6", "removal_pending": true},
            ]},
            {"node_id": phone, "boards": [
                {"handle": "3f9a12c4d5e6", "removal_pending": false},
            ]},
        ])
    );
}

#[cfg(test)]
#[test]
fn a_drop_reports_whether_the_relay_deletes_copies() {
    let kept = Dropped(pepino_node::DroppedBoards {
        boards: 2,
        purged: 0,
    });
    assert_eq!(
        kept.consequence(),
        "Other devices still request them, so the relay keeps its copies."
    );
    let purged = Dropped(pepino_node::DroppedBoards {
        boards: 3,
        purged: 1,
    });
    assert_eq!(
        purged.consequence(),
        "The relay restarts its node and deletes 1 stored copies."
    );
    let fields = purged.json_fields();
    assert_eq!(
        (&fields["dropped"], &fields["purged"]),
        (&json!(3), &json!(1))
    );
}
