use pepino_app::{OpenOptions, ReadRequest, Request};
use ppn::app::AppClient;
use ppn::daemon::{address::socket_path, client::Client, network::membership, protocol::Network};
use ppn::error::{CliError, shorten_message_ids};
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::{short_id, single_line};
use ppn::response::{ResponseExt, Status};
use ppn::terminal::{Context, RenderSettings};

#[derive(clap::Args)]
/// Show local identity, board, task counts, and sync activity.
#[command(after_help = example_help(&["ppn status", "ppn status --json"]))]
pub struct Args {}

pub fn run(context: &Context<'_>, _: Args) -> Result<CommandResult, CliError> {
    let client = AppClient::open(context)?;
    let path = socket_path(&context.config()?.config);
    let network = (Client { path })
        .status()
        .ok()
        .flatten()
        .map(|status| status.network);
    document(context, client, network)
}

#[derive(clap::Args)]
#[command(about = "Initialise the local identity, device, and board.", after_help = example_help(&[
    "ppn init --name 'Ada Gardener' --device 'garden laptop' --board Greenhouse",
    "ppn init --json",
]))]
pub struct InitArgs {
    /// Display name for the local identity.
    #[arg(long, value_name = "NAME")]
    name: Option<String>,
    /// Name for this device.
    #[arg(long, value_name = "NAME")]
    device: Option<String>,
    /// Name for the local board.
    #[arg(long, value_name = "NAME")]
    board: Option<String>,
}

pub fn init(context: &Context<'_>, args: InitArgs) -> Result<CommandResult, CliError> {
    let options = OpenOptions {
        display_name: args.name,
        device_name: args.device,
        board_name: args.board,
        ..OpenOptions::default()
    };
    document(context, AppClient::init(context, options)?, None)
}

fn document(
    context: &Context<'_>,
    mut client: AppClient,
    network: Option<Network>,
) -> Result<CommandResult, CliError> {
    let response = client.call(Request::Read(ReadRequest::Status))?;
    let local = client.local_state();
    let boards = local
        .boards()
        .iter()
        .map(|board| {
            let (state, sync) = super::board::board_activity(
                board.id,
                local.board_join_state(board.id),
                network.as_ref(),
            );
            (board.clone(), state, sync)
        })
        .collect::<Vec<_>>();
    let membership_rows = boards
        .iter()
        .map(|(board, state, sync)| {
            format!(
                "{}: {} ({})",
                single_line(&board.name),
                membership(*state),
                sync.replace('_', " ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n  ");
    let (selected, source) = client.selected_board()?;
    let selected_line =
        super::board::selection_line(&selected.name, &selected.id.to_string(), source);
    let selected = serde_json::json!({"id":selected.id, "name":selected.name, "source":source});
    let settings = context.render_settings()?;
    let human_network = network
        .as_ref()
        .map(|network| describe_network(network, &settings));
    CommandResult::app_document_with_json(
        context,
        response,
        move |response, _| {
            let Status {
                identity,
                device,
                board,
                data_dir,
                credentials_file,
                network: _,
                counts,
                protocol_version,
            } = response.status()?;
            let skipped = &counts.skipped;
            let network = human_network
                .as_deref()
                .unwrap_or("offline (daemon not running)");
            Ok(format!(
                "Identity: {} ({})\nDevice: {} (node ID {})\nBoard: {} ({})\n{selected_line}\nMembership:\n  {membership_rows}\nData directory: {}\nCredentials: {}\nnetwork: {network}\nTasks: {} open, {} closed\nEvents: {} total, {} applied\nSkipped events: {} unknown version, {} unknown event, {} unknown value, {} invalid, {} forged author\nProtocol: v{}",
                single_line(&identity.display_name),
                short_id(&identity.id),
                single_line(&device.name),
                short_id(&device.id),
                single_line(&board.name),
                short_id(&board.id),
                single_line(data_dir),
                single_line(credentials_file),
                counts.open,
                counts.closed,
                counts.events,
                counts.applied,
                skipped.unknown_version,
                skipped.unknown_event,
                skipped.unknown_value,
                skipped.invalid,
                skipped.forged_author,
                protocol_version,
            ))
        },
        move |fields| {
            let boards = boards.into_iter().map(|(board, state, sync)| {
                serde_json::json!({"id": board.id, "name": board.name, "membership": state, "sync": sync})
            }).collect::<Vec<_>>();
            fields.insert("boards".into(), serde_json::json!(boards));
            fields.insert("selected_board".into(), selected);
            if let Some(network) = network {
                fields.insert("network".into(), serde_json::json!(network));
            }
        },
    )
}

fn describe_network(network: &Network, settings: &RenderSettings) -> String {
    use std::fmt::Write;

    let mut text = format!(
        "{} peers seen, {} connected",
        network.peers_seen.len(),
        network.peers_connected.len()
    );
    for (id, board) in &network.boards {
        let pending = board
            .events_waiting
            .map_or_else(|| "unknown".into(), |count| count.to_string());
        let time = |time: Option<chrono::DateTime<chrono::Utc>>| {
            time.map_or_else(
                || "none".into(),
                |time| settings.dates().timestamp(time, &settings.terminal),
            )
        };
        let starting = if board.starting_up {
            "starting up, "
        } else {
            ""
        };
        let error = board.last_error.as_deref().map(describe_sync_error);
        let error = error.as_deref().unwrap_or("None.");
        let _ = write!(
            text,
            "\n  Board {}: {starting}{pending} events waiting to be sent, last sent {}, last received {}, Last error: {error}",
            short_id(id),
            time(board.last_sent),
            time(board.last_received)
        );
    }
    text
}

fn describe_sync_error(error: &str) -> String {
    let (action, reason) = error.split_once(": ").unwrap_or((error, ""));
    let action = shorten_message_ids(&single_line(action));
    let reason = shorten_message_ids(&single_line(reason));
    if reason.is_empty() {
        format!("Sync failed: {}.", action.trim_end_matches('.'))
    } else {
        format!("Could not {action}: {}.", reason.trim_end_matches('.'))
    }
}

#[cfg(test)]
#[test]
fn human_sync_errors_use_short_ids_and_sentences() {
    let id = "03".repeat(32);
    let message = describe_sync_error(&format!("publish {id}: the operation was not submitted"));
    assert_eq!(
        message,
        "Could not publish 030303: the operation was not submitted."
    );
    assert!(!message.contains(&id));
}
