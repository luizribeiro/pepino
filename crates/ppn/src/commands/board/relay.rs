use pepino_app::{BoardRelay, LocalState, PeerHint};
use pepino_core::{BoardId, DeviceId};
use ppn::{
    app::AppClient,
    error::{CliError, ErrorKind},
    help_tree::example_help,
    output::{CommandResult, Render},
    render::single_line,
    terminal::{Context, Terminal},
};
use serde::Serialize;
use serde_json::{Map, Value, json};

#[derive(clap::Args)]
#[command(about = "Hand encrypted boards to trusted relay nodes.", after_help = example_help(&["ppn board relay ls --board Greenhouse"]))]
pub struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Ask a relay to carry the board's encrypted history.
    #[command(after_help = example_help(&["ppn board relay add NODE_ID@https://relay.example --board Greenhouse"]))]
    Add {
        /// Relay node ID, with an optional @HTTPS_URL suffix.
        #[arg(value_name = "RELAY")]
        relay: String,
        /// Request carrying for every local board.
        #[arg(long, conflicts_with = "board")]
        all_boards: bool,
    },
    /// Ask a relay to stop carrying the board and delete its copy.
    #[command(after_help = example_help(&["ppn board relay rm NODE_ID --all-boards --yes"]))]
    Rm {
        /// Relay node ID, with an optional @HTTPS_URL suffix.
        #[arg(value_name = "RELAY")]
        relay: String,
        /// Request removal for every local board.
        #[arg(long, conflicts_with = "board")]
        all_boards: bool,
    },
    /// Show the relay requests and confirmed state for a board.
    #[command(after_help = example_help(&["ppn board relay ls --board Greenhouse --json"]))]
    Ls,
}

impl Args {
    pub(super) fn writes(&self) -> bool {
        !matches!(self.action, Action::Ls)
    }
}

pub fn run(context: &Context<'_>, args: Args) -> Result<CommandResult, CliError> {
    let client = AppClient::open(context)?;
    let (relay, all_boards, carry) = match args.action {
        Action::Add { relay, all_boards } => (relay, all_boards, true),
        Action::Rm { relay, all_boards } => (relay, all_boards, false),
        Action::Ls => {
            let selected = client.selected_board()?.0.id;
            return Ok(CommandResult::document(list(
                client.local_state(),
                &[selected],
                None,
            )));
        }
    };
    let peer = peer_hint(&relay)?;
    let boards: Vec<BoardId> = if all_boards {
        client
            .local_state()
            .boards()
            .iter()
            .map(|board| board.id)
            .collect()
    } else {
        vec![client.selected_board()?.0.id]
    };
    if !carry && !context.confirm("Remove the relay's stored copy of the selected boards?")? {
        return Err(CliError::new(
            ErrorKind::ConfirmationRequired,
            "Relay removal was not confirmed.",
        ));
    }
    let id = peer.id;
    let local = client.set_board_relay(boards.clone(), peer, carry)?;
    Ok(CommandResult::document(list(&local, &boards, Some(id))))
}

fn peer_hint(target: &str) -> Result<PeerHint, CliError> {
    let (id, url) = target
        .split_once('@')
        .map_or((target, None), |(id, url)| (id, Some(url)));
    let id = id.parse::<DeviceId>().map_err(|_| {
        CliError::new(
            ErrorKind::InvalidInput,
            "Expected a 64-digit relay node ID.",
        )
    })?;
    let relay_url = url
        .map(pepino_node::peer_relay_url)
        .transpose()
        .map_err(|_| {
            CliError::new(
                ErrorKind::InvalidInput,
                "Expected an HTTPS URL after the relay node ID.",
            )
        })?
        .map(|url| url.to_string());
    Ok(PeerHint { id, relay_url })
}

#[derive(Clone, Serialize)]
pub(super) struct Entry {
    relay: DeviceId,
    state: pepino_app::RelayState,
    requested_by: DeviceId,
    relay_url: Option<String>,
    reason: Option<String>,
    carry: bool,
}

impl Entry {
    fn new(relay: DeviceId, entry: &BoardRelay) -> Self {
        Self {
            relay,
            state: entry.state,
            requested_by: entry.revision.writer,
            relay_url: entry.relay_url.clone(),
            reason: entry.reason.clone(),
            carry: entry.carry,
        }
    }

    pub(super) fn human(&self) -> String {
        let reason = self.reason.as_ref().map_or(String::new(), |reason| {
            format!(" ({})", single_line(reason))
        });
        let removal = !self.carry
            && matches!(
                self.state,
                pepino_app::RelayState::Requested
                    | pepino_app::RelayState::Unreachable
                    | pepino_app::RelayState::Refused
            );
        let operation = if removal { "removal " } else { "" };
        format!("{}  {operation}{}{reason}", self.relay, self.state)
    }
}

pub(super) fn entries(local: &LocalState, board: BoardId) -> Vec<Entry> {
    local
        .board_relays(board)
        .map(|(relay, entry)| Entry::new(relay, entry))
        .collect()
}

#[derive(Serialize)]
struct Board {
    board: String,
    relays: Vec<Entry>,
}

struct List(Vec<Board>);

fn list(local: &LocalState, boards: &[BoardId], relay: Option<DeviceId>) -> List {
    List(
        local
            .boards()
            .iter()
            .filter(|board| boards.contains(&board.id))
            .map(|board| {
                let mut relays = entries(local, board.id);
                relays.retain(|entry| relay.is_none_or(|relay| entry.relay == relay));
                Board {
                    board: board.name.clone(),
                    relays,
                }
            })
            .collect(),
    )
}

impl Render for List {
    fn human(&self, _: &Terminal) -> String {
        let mut lines = Vec::new();
        for board in &self.0 {
            lines.push(single_line(&board.board));
            if board.relays.is_empty() {
                lines.push("  No relay requests".into());
            }
            lines.extend(
                board
                    .relays
                    .iter()
                    .map(|entry| format!("  {}", entry.human())),
            );
        }
        lines.join("\n")
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), json!("board_relays")),
            ("boards".into(), json!(self.0)),
        ])
    }
}

#[cfg(test)]
#[test]
fn a_copy_kept_for_another_device_is_listed_as_such() {
    let relay = DeviceId::from_bytes([31; 32]);
    let entry = Entry::new(
        relay,
        &BoardRelay {
            revision: pepino_node::RelayRevision {
                sequence: 1_795_300_000_000,
                writer: DeviceId::from_bytes([74; 32]),
            },
            state: pepino_app::RelayState::StillCarried,
            carry: false,
            relay_url: None,
            reason: None,
        },
    );
    assert_eq!(
        entry.human(),
        format!("{relay}  removed for this device; the relay still carries it for another device")
    );
    assert_eq!(json!(entry)["state"], "still_carried");
}
