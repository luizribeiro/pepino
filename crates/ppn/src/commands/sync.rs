use std::{path::Path, time::Duration};

use ppn::{
    daemon::{
        address::socket_path,
        client::Client,
        protocol::{Operation, Outcome, wait_millis},
        sync_report::Summary,
    },
    error::{CliError, ErrorKind},
    help_tree::example_help,
    output::CommandResult,
    terminal::Context,
};

use super::daemon::{ensure_running, unreachable};

#[derive(clap::Args)]
/// Publish local writes and wait for the daemon to finish known sync work.
#[command(after_help = example_help(&["ppn sync --timeout 10s", "ppn sync --json"]))]
pub struct Args {}

pub fn run(
    context: &Context<'_>,
    _: Args,
    selection: Option<&Path>,
    enabled: bool,
) -> Result<CommandResult, CliError> {
    let wait = context.timeout.unwrap_or(Duration::from_secs(30));
    let timeout_ms = wait_millis(wait)?;
    let config = &context.config()?.config;
    let client = Client {
        path: socket_path(config),
    };
    let before = client.status().ok().flatten();
    ensure_running(
        &client,
        context,
        selection,
        enabled && config.daemon.autostart.value,
    )?;
    let operation = Operation::WaitIdle { timeout_ms };
    let outcome = client
        .call_with(operation, wait.saturating_add(Duration::from_secs(2)))
        .map_err(unreachable)?;
    let (idle, mut network, peer_syncs) = match outcome {
        Outcome::Synced {
            idle,
            network,
            peer_syncs,
        } => (idle, network, peer_syncs),
        Outcome::Error { message, .. } => return Err(CliError::new(ErrorKind::Failure, message)),
        _ => return Err(protocol_error()),
    };
    for (id, board) in &mut network.boards {
        if let Some(previous) = before
            .as_ref()
            .and_then(|state| state.network.boards.get(id))
        {
            board.published = board.published.saturating_sub(previous.published);
            board.received = board.received.saturating_sub(previous.received);
        }
        if board.published == 0 {
            board.last_sent = None;
        }
        if board.received == 0 {
            board.last_received = None;
        }
    }
    network.peers_synced.retain(|id| {
        id.parse::<pepino_core::DeviceId>().ok().is_some_and(|id| {
            let count = peer_syncs.get(&id).copied().unwrap_or_default();
            let seen = before
                .as_ref()
                .and_then(|state| state.peer_syncs.get(&id))
                .copied()
                .unwrap_or_default();
            count > seen
        })
    });
    let devices = pepino_app::LocalState::load(config.data_dir.value.as_path()).map_or_else(
        |_| Default::default(),
        |local| {
            local
                .boards()
                .iter()
                .flat_map(|board| local.author_devices(board.id))
                .map(|(device, (_, name))| (device, name))
                .collect()
        },
    );
    let settings = context.render_settings()?;
    let describe_time = |time| settings.dates().timestamp(time, &settings.terminal);
    let summary = Summary {
        last_sent: network
            .boards
            .values()
            .filter_map(|board| board.last_sent)
            .max()
            .map(describe_time),
        last_received: network
            .boards
            .values()
            .filter_map(|board| board.last_received)
            .max()
            .map(describe_time),
        network,
        devices,
    };
    if !idle {
        return Err(CliError::new(
            ErrorKind::SyncTimeout,
            format!("Sync timed out with work pending. {}", summary.describe()),
        ));
    }
    Ok(CommandResult::document(summary))
}

fn protocol_error() -> CliError {
    CliError::new(
        ErrorKind::DaemonUnreachable,
        "The daemon returned an unexpected control response.",
    )
}
