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
    ensure_running(
        &client,
        context,
        selection,
        enabled && config.daemon.autostart.value,
    )?;
    if !matches!(
        client.call(Operation::Nudge).map_err(unreachable)?,
        Outcome::Nudged
    ) {
        return Err(protocol_error());
    }
    let operation = Operation::WaitIdle { timeout_ms };
    let outcome = client
        .call_with(operation, wait.saturating_add(Duration::from_secs(2)))
        .map_err(unreachable)?;
    let Outcome::Synced { idle, network } = outcome else {
        return Err(protocol_error());
    };
    let summary = Summary(network);
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
