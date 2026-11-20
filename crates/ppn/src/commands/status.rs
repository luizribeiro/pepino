use pepino_app::{OpenOptions, ReadRequest, Request};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::{short_id, single_line};
use ppn::response::{ResponseExt, Status};
use ppn::terminal::Context;

#[derive(clap::Args)]
/// Show local identity, board, and task counts.
#[command(after_help = example_help(&["ppn status", "ppn status --json"]))]
pub struct Args {}

pub fn run(context: &Context<'_>, _: Args) -> Result<CommandResult, CliError> {
    document(context, AppClient::open(context)?)
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
    document(context, AppClient::init(context, options)?)
}

fn document(context: &Context<'_>, mut client: AppClient) -> Result<CommandResult, CliError> {
    let response = client.call(Request::Read(ReadRequest::Status))?;
    CommandResult::app_document(context, response, |response, _| {
        let Status {
            identity,
            device,
            board,
            data_dir,
            credentials_file,
            network,
            counts,
            protocol_version,
        } = response.status()?;
        let skipped = &counts.skipped;
        let network = match network {
            pepino_app::NetworkStatus::Offline => "offline (daemon not running)",
        };
        Ok(format!(
            "Identity: {} ({})\nDevice: {} (node ID {})\nBoard: {} ({})\nData directory: {}\nCredentials: {}\nnetwork: {network}\nTasks: {} open, {} closed\nEvents: {} total, {} applied\nSkipped events: {} unknown version, {} unknown event, {} unknown value, {} invalid, {} forged author\nProtocol: v{}",
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
    })
}
