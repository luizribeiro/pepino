use std::path::Path;

use pepino_app::{ReadRequest, Request, Response, WriteRequest};
use ppn::{
    app::AppClient,
    daemon::{address::socket_path, client::Client, network::membership, protocol::Network},
    error::{CliError, ErrorKind},
    help_tree::example_help,
    output::{CommandResult, Warning},
    render::{ShortIds, single_line},
    terminal::Context,
};
use serde_json::json;

#[derive(clap::Args)]
#[command(about = "Manage this identity's boards.", after_help = example_help(&["ppn board ls", "ppn board show Workshop"]))]
pub struct Args {
    #[command(subcommand)]
    operation: Operation,
}

#[derive(clap::Subcommand)]
enum Operation {
    #[command(about = "List this identity's boards.", after_help = example_help(&["ppn board ls --json"]))]
    Ls,
    #[command(about = "Show details for a board.", after_help = example_help(&["ppn board show Workshop"]))]
    Show {
        /// Board name or unambiguous ID prefix. Defaults to the selected board.
        #[arg(value_name = "BOARD")]
        reference: Option<String>,
    },
    #[command(about = "Set the host's current board.", after_help = example_help(&["ppn board use Workshop"]))]
    Use {
        /// Board name or unambiguous ID prefix.
        #[arg(value_name = "BOARD")]
        reference: String,
    },
    #[command(about = "Create an encrypted board.", after_help = example_help(&["ppn board new Workshop"]))]
    New {
        /// Name for the new board.
        name: String,
    },
    #[command(about = "Rename a board on all linked devices.", after_help = example_help(&["ppn board rename Workshop 'Tool Shed'"]))]
    Rename {
        /// Board name or unambiguous ID prefix.
        #[arg(value_name = "BOARD")]
        reference: String,
        /// New name for the board.
        name: String,
    },
}

pub fn run(
    context: &Context<'_>,
    args: Args,
    config_path: Option<&Path>,
    allow_start: bool,
) -> Result<CommandResult, CliError> {
    let mut client = AppClient::open(context)?;
    let using = matches!(args.operation, Operation::Use { .. });
    let (response, show_selection) = match args.operation {
        Operation::Ls => (client.call(Request::Read(ReadRequest::BoardList))?, true),
        Operation::Show { reference: board } => (
            client.call(Request::Read(ReadRequest::BoardShow { board }))?,
            true,
        ),
        Operation::Use { reference: board } => (
            client.call_unscoped(Request::Write(WriteRequest::BoardUse { board }))?,
            false,
        ),
        Operation::New { name } => {
            let board = create(context, &name, config_path, allow_start)?;
            client = AppClient::open(context)?;
            (
                client
                    .call_unscoped(Request::Read(ReadRequest::BoardShow { board: Some(board) }))?,
                false,
            )
        }
        Operation::Rename {
            reference: board,
            name,
        } => (
            client.call_unscoped(Request::Write(WriteRequest::BoardRename { board, name }))?,
            false,
        ),
    };
    let selection = show_selection
        .then(|| client.selected_board())
        .transpose()?;
    let network = daemon_network(context)?;
    let mut result = document(context, response, selection, network.as_ref())?;
    if using
        && context
            .board_env
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    {
        result = result.with_warnings([Warning {
            message: "PPN_BOARD still overrides the current board in this shell.".into(),
            details: serde_json::Map::from_iter([("code".into(), json!("board_overridden"))]),
        }]);
    }
    Ok(result)
}

pub(super) fn daemon_network(context: &Context<'_>) -> Result<Option<Network>, CliError> {
    let path = socket_path(&context.config()?.config);
    Ok((Client { path })
        .status()
        .ok()
        .flatten()
        .map(|status| status.network))
}

fn document(
    context: &Context<'_>,
    response: Response,
    selection: Option<(&pepino_app::LocalBoard, &str)>,
    network: Option<&Network>,
) -> Result<CommandResult, CliError> {
    let boards = match &response {
        Response::Board { board } => std::slice::from_ref(board),
        Response::Boards { boards } => boards,
        _ => return Err(CliError::new(ErrorKind::Failure, "Expected board details.")),
    };
    let ids: Vec<_> = boards
        .iter()
        .map(|info| info.board.id.to_string())
        .collect();
    let short = ShortIds::new(ids.iter().map(String::as_str));
    let mut rows = Vec::new();
    let mut records = Vec::new();
    for (info, id) in boards.iter().zip(&ids) {
        let (state, sync) = board_activity(info.board.id, info.membership, network);
        let activity = sync.replace('_', " ");
        rows.push(format!(
            "{} {} ({})  {}/{} open  {}  {activity}",
            if info.current { "*" } else { " " },
            single_line(&info.board.name),
            short.get(id),
            info.open,
            info.total,
            membership(state),
        ));
        let mut record = json!(info);
        record["membership"] = json!(state);
        record["sync"] = json!(sync);
        records.push(record);
    }
    let selected = selection.map(|(board, source)| {
        rows.push(selection_line(&board.name, &board.id.to_string(), source));
        json!({"id":board.id,"name":board.name,"source":source})
    });
    CommandResult::app_document_with_json(
        context,
        response,
        move |_, _| Ok(rows.join("\n")),
        move |fields| {
            if fields.contains_key("board") {
                fields.insert("board".into(), json!(records[0]));
            } else {
                fields.insert("boards".into(), json!(records));
            }
            if let Some(selected) = selected {
                fields.insert("selected_board".into(), selected);
            }
        },
    )
}

pub(super) fn selection_line(name: &str, id: &str, source: &str) -> String {
    format!(
        "Selected board: {} ({}) via {source}",
        single_line(name),
        ppn::render::short_id(id)
    )
}

pub(super) fn board_activity(
    id: pepino_core::BoardId,
    membership: pepino_app::BoardJoinState,
    network: Option<&Network>,
) -> (pepino_app::BoardJoinState, &'static str) {
    let state = network
        .and_then(|network| network.board_joins.get(&id).copied())
        .unwrap_or(membership);
    let sync = match network.and_then(|network| network.boards.get(&id.to_string())) {
        Some(board) if board.last_error.is_some() => "error",
        Some(board) if board.starting_up => "starting_up",
        Some(board)
            if board.events_waiting == Some(0) && state == pepino_app::BoardJoinState::Joined =>
        {
            "idle"
        }
        Some(_) => "pending",
        None if network.is_some() => "pending",
        None => "offline",
    };
    (state, sync)
}

fn create(
    context: &Context<'_>,
    name: &str,
    config_path: Option<&Path>,
    allow_start: bool,
) -> Result<String, CliError> {
    use ppn::daemon::protocol::{Operation, Outcome};

    pepino_app::App::validate_board_name(name)?;
    let config = &context.config()?.config;
    let client = Client {
        path: socket_path(config),
    };
    super::daemon::ensure_running(
        &client,
        context,
        config_path,
        allow_start && config.daemon.autostart.value,
    )?;
    let wait = context
        .timeout
        .unwrap_or(std::time::Duration::from_secs(35));
    match client
        .call_with(Operation::CreateBoard { name: name.into() }, wait)
        .map_err(super::daemon::unreachable)?
    {
        Outcome::BoardCreated { board } => Ok(board.to_string()),
        _ => Err(CliError::new(
            ErrorKind::Failure,
            "Could not create the board.",
        )),
    }
}
