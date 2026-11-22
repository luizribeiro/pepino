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
    let mut already_stopped = false;
    let (action, status) = match args.action {
        Action::Run => {
            let detached = process::detach().map_err(unreachable)?;
            if let Err(error) = runtime::run(config, level)
                && !(detached && error.kind() == ErrorKind::Busy)
            {
                return Err(error);
            }
            ("stopped", None)
        }
        Action::Start => (
            "started",
            Some(ensure_running(&client, context, selection, true)?),
        ),
        Action::Stop => {
            already_stopped =
                !process::stop(&client, context.timeout.unwrap_or(process::STOP_LIMIT))
                    .map_err(unreachable)?;
            ("stopped", None)
        }
        Action::Status => {
            let status = client.status().map_err(unreachable)?;
            if status.is_none() {
                check_directory(context)?;
            }
            ("status", status)
        }
    };
    Ok(CommandResult::document(State {
        action,
        already_stopped,
        status,
        socket_path: client.path,
    }))
}

fn check_directory(context: &Context<'_>) -> Result<(), CliError> {
    let config = &context.config()?.config;
    let data = config.data_dir.value.as_path();
    if pepino_app::is_relay(data)? {
        pepino_app::relay_credentials(data, config.credentials_file.value.as_path())?;
    } else {
        drop(AppClient::open(context)?);
    }
    Ok(())
}

pub fn ensure_running(
    client: &Client,
    context: &Context<'_>,
    selection: Option<&Path>,
    allow_start: bool,
) -> Result<DaemonStatus, CliError> {
    match client.status() {
        Ok(Some(status)) if status.network.starting_ms.is_some() => {
            return process::wait_for_service(client, process::START_LIMIT).map_err(unreachable);
        }
        Ok(Some(status)) => return Ok(status),
        Ok(None) => (),
        Err(error)
            if allow_start
                && matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::TimedOut
                ) => {}
        Err(error) => return Err(unreachable(error)),
    }
    check_directory(context)?;
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
