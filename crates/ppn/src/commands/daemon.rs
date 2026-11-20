use clap::Subcommand;
use ppn::{
    app::AppClient,
    daemon::{address::socket_path, client::Client, process, report::State, runtime},
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
    /// Report the local sync daemon's process and socket.
    #[command(after_help = example_help(&["ppn daemon status --json"]))]
    Status,
}

pub fn run(context: &Context<'_>, args: Args) -> Result<CommandResult, CliError> {
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
        Action::Status => {
            drop(AppClient::open(context)?);
            ("status", client.status().map_err(unreachable)?)
        }
    };
    Ok(CommandResult::document(State {
        action,
        status,
        socket_path: client.path,
    }))
}

pub fn unreachable(error: std::io::Error) -> CliError {
    CliError::new(
        ErrorKind::DaemonUnreachable,
        format!("Could not reach the daemon. {error}"),
    )
}
