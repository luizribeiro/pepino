use std::path::Path;

use clap::Subcommand;
use ppn::{
    app::AppClient,
    daemon::{
        address::socket_path,
        client::{Client, unavailable},
        process,
        protocol::DaemonStatus,
        report::State,
        runtime,
    },
    error::{CliError, ErrorKind},
    help_tree::example_help,
    output::CommandResult,
    terminal::Context,
};

#[derive(clap::Args)]
#[command(about = "Manage the local sync daemon.", after_help = example_help(&["ppn daemon status"]))]
pub struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Run the sync daemon in the foreground.
    #[command(after_help = example_help(&["ppn daemon run", "ppn daemon run --log-level warn"]))]
    Run,
    /// Start the sync daemon in the background and wait for its control socket.
    #[command(after_help = example_help(&["ppn daemon start --json"]))]
    Start,
    /// Stop the local sync daemon and wait for shutdown.
    #[command(after_help = example_help(&["ppn daemon stop"]))]
    Stop,
    /// Report the local sync daemon's process and socket.
    #[command(after_help = example_help(&["ppn daemon status --json"]))]
    Status,
}

pub fn run(
    context: &Context<'_>,
    args: Args,
    selection: Option<&Path>,
) -> Result<CommandResult, CliError> {
    let config = &context.config()?.config;
    let level = config.log_level.value;
    let client = Client {
        path: socket_path(config),
    };
    let (action, status) = match args.action {
        Action::Run => {
            process::detach().map_err(unreachable)?;
            runtime::run(config, level)?;
            ("stopped", None)
        }
        Action::Start => (
            "started",
            Some(ensure_running(&client, context, selection, true)?),
        ),
        Action::Stop => {
            if let Err(error) =
                process::stop(&client, context.timeout.unwrap_or(process::STOP_LIMIT))
            {
                if unavailable(&error) {
                    drop(AppClient::open(context)?);
                }
                return Err(unreachable(error));
            }
            ("stopped", None)
        }
        Action::Status => {
            let status = client.status().map_err(unreachable)?;
            if status.is_none() {
                drop(AppClient::open(context)?);
            }
            ("status", status)
        }
    };
    Ok(CommandResult::document(State {
        action,
        status,
        socket_path: client.path,
    }))
}

pub fn ensure_running(
    client: &Client,
    context: &Context<'_>,
    selection: Option<&Path>,
    allow_start: bool,
) -> Result<DaemonStatus, CliError> {
    if let Some(status) = client.status().map_err(unreachable)? {
        return Ok(status);
    }
    drop(AppClient::open(context)?);
    if !allow_start {
        return Err(CliError::new(
            ErrorKind::DaemonUnreachable,
            "The daemon is not running and automatic startup is disabled.",
        ));
    }
    let config = &context.config()?.config;
    let child = process::spawn(config, selection, config.log_level.value).map_err(unreachable)?;
    process::wait_started(
        client,
        config.data_dir.value.as_path(),
        child,
        process::START_LIMIT,
    )
    .map_err(unreachable)
}

pub fn unreachable(error: std::io::Error) -> CliError {
    CliError::new(
        ErrorKind::DaemonUnreachable,
        format!("Could not reach the daemon. {error}"),
    )
}

pub fn notify_write(context: &Context<'_>, selection: Option<&Path>, verbose: bool) {
    use std::io::Write as _;

    let Ok(loaded) = context.config() else { return };
    let config = &loaded.config;
    if !config.daemon.autostart.value {
        return;
    }
    let client = Client {
        path: socket_path(config),
    };
    let result = client.nudge().or_else(|error| {
        if !unavailable(&error) {
            return Err(error);
        }
        let child = process::spawn(config, selection, config.log_level.value)?;
        process::nudge_on_startup(&client, child)
    });
    if let Err(error) = result
        && verbose
    {
        let _ = writeln!(
            std::io::stderr().lock(),
            "Could not notify the daemon. {error}"
        );
    }
}
