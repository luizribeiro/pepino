use std::{path::Path, time::Duration};

use pepino_app::{BoardDeparture, MUTUAL_REMOVAL_RISK, ManagePerson, RelayCopy};
use pepino_core::DeviceId;
use pepino_node::{MemberState, Node};
use ppn::{
    app::AppClient,
    daemon::protocol::{Operation, Outcome},
    error::{CliError, ErrorKind},
    output::{CommandResult, Render},
    render::single_line,
    terminal::{Context, Terminal},
};
use serde_json::{Map, Value, json};

use crate::commands::{
    daemon::unreachable,
    device::{runtime, unexpected_reply},
};

const REMOVAL: &str = "They keep the data they already had, and the transport can still deliver ciphertext. An offline device does not learn of the removal from the board. Re-inviting gives them the full history, including work added while they were away.";
const LEAVE: &str = "Tasks, comments, and other data already synced stay on this device. After departure, run ppn board forget on each device where you want to delete the local history.";

pub(super) fn run(
    context: &Context<'_>,
    config_path: Option<&Path>,
    allow_start: bool,
    reference: &str,
    action: ManagePerson,
    acknowledge: bool,
) -> Result<CommandResult, CliError> {
    let app = AppClient::open(context)?;
    let board = app.selected_board()?.0;
    let local = app.local_state();
    local.check_invitable(board.id)?;
    let person = local.resolve_board_person(board.id, reference)?;
    let name = local
        .person_labels()
        .remove(&person)
        .unwrap_or_else(|| reference.to_owned());
    let previous = local.board_departure(board.id);
    let wait = context.timeout.unwrap_or(Duration::from_secs(30));
    let daemon = if matches!(action, ManagePerson::Leave) && previous.is_none() {
        Some(super::request::running(context, config_path, allow_start)?)
    } else {
        None
    };
    let risk = if let Some(daemon) = &daemon {
        let timeout_ms = ppn::daemon::protocol::wait_millis(wait)?;
        match daemon
            .call_with(
                Operation::BoardLeaveCheck {
                    board: board.id,
                    person,
                    timeout_ms,
                },
                wait.saturating_add(Duration::from_secs(2)),
            )
            .map_err(unreachable)?
        {
            Outcome::BoardLeaveChecked { risk } => risk,
            Outcome::Error { code, message } => {
                return Err(super::request::control_error(&code, message));
            }
            _ => return Err(unexpected_reply()),
        }
    } else if matches!(action, ManagePerson::Leave) {
        false
    } else {
        let path = context
            .config()?
            .config
            .data_dir
            .value
            .as_path()
            .join("node.sqlite");
        let members = runtime()?
            .block_on(Node::persisted_members(&path, board.id))
            .map_err(|_| CliError::new(ErrorKind::Failure, "Could not read the board membership."))?
            .into_iter()
            .filter_map(|(id, state)| match state {
                MemberState::Member(role) => Some((id, role)),
                _ => None,
            })
            .collect();
        let devices = local.membership_devices(board.id, person);
        action.check(local, board.id, person, &devices, &members, true)?;
        action.requires_risk_ack(local, board.id, person, &devices, &members)
    };
    if risk && !(context.yes && acknowledge) {
        let details = if matches!(action, ManagePerson::Remove) {
            format!(" {REMOVAL}")
        } else {
            String::new()
        };
        return Err(CliError::new(
            ErrorKind::ConfirmationRequired,
            format!("{MUTUAL_REMOVAL_RISK}{details}"),
        )
        .with_hint("Pass --yes and --i-understand-mutual-removal-risk to confirm."));
    }
    let (kind, prompt, notice) = match action {
        ManagePerson::Role(_) => ("board_role", None, None),
        ManagePerson::Remove => (
            "board_remove",
            Some(format!(
                "Remove {} from {}? {REMOVAL}",
                single_line(&name),
                single_line(&board.name)
            )),
            Some(REMOVAL),
        ),
        ManagePerson::Leave => (
            "board_leave",
            Some(format!(
                "Leave {} on all your devices? {LEAVE}",
                single_line(&board.name)
            )),
            Some(LEAVE),
        ),
    };
    let confirmed = match prompt {
        Some(prompt) if previous.is_none() => context.confirm(&prompt)?,
        _ => true,
    };
    let departure = if !confirmed {
        None
    } else if matches!(action, ManagePerson::Leave) && previous.is_some() {
        if local.board_departure_confirmed(board.id) {
            previous
        } else {
            Some(BoardDeparture::Leaving)
        }
    } else {
        let daemon = match daemon {
            Some(daemon) => daemon,
            None => super::request::running(context, config_path, allow_start)?,
        };
        match daemon
            .call_with(
                Operation::BoardManage {
                    board: board.id,
                    person,
                    action,
                    accept_risk: context.yes && acknowledge,
                },
                wait,
            )
            .map_err(unreachable)?
        {
            Outcome::BoardManaged { departure } => departure,
            Outcome::Error { code, message } => {
                return Err(super::request::control_error(&code, message));
            }
            _ => return Err(unexpected_reply()),
        }
    };
    let human = if !confirmed {
        "Membership change cancelled.".into()
    } else {
        let board_name = single_line(&board.name);
        let name = single_line(&name);
        match action {
            ManagePerson::Role(role) => {
                format!("{name} has the {role} role on {board_name}.")
            }
            ManagePerson::Remove => {
                format!("Removed {name} from {board_name}. {REMOVAL}")
            }
            ManagePerson::Leave => {
                let state = departure.unwrap_or(BoardDeparture::Leaving);
                let pending = if state == BoardDeparture::Leaving {
                    " · membership change incomplete"
                } else {
                    ""
                };
                format!("{board_name} · {state}{pending}. {LEAVE}")
            }
        }
    };
    Ok(CommandResult::document(ResultDocument {
        human,
        fields: Map::from_iter([
            ("kind".into(), json!(kind)),
            ("board".into(), json!(board)),
            (
                "person".into(),
                json!({"id": person.0, "root": person.1, "name": name}),
            ),
            (
                "role".into(),
                json!(if let ManagePerson::Role(role) = action {
                    Some(role)
                } else {
                    None
                }),
            ),
            ("cancelled".into(), json!(!confirmed)),
            (
                "complete".into(),
                json!(confirmed && departure != Some(BoardDeparture::Leaving)),
            ),
            ("departure".into(), json!(departure)),
            ("consequences".into(), json!(notice)),
        ]),
    }))
}

pub(super) fn forget(context: &Context<'_>, reference: &str) -> Result<CommandResult, CliError> {
    let mut app = AppClient::open(context)?;
    let board = app.local_state().resolve_board(reference, "board")?.clone();
    app.local_state().check_can_forget(board.id)?;
    let confirmed = context.confirm(&format!(
        "Delete the local task and membership history for {}?",
        single_line(&board.name)
    ))?;
    let mut kept = Vec::new();
    if confirmed {
        let directory = &context.config()?.config.data_dir.value;
        let _lease = ppn::daemon::socket::lease(&directory.as_path().join("daemon.lock")).map_err(
            |error| {
                if error.kind() == std::io::ErrorKind::AddrInUse {
                    CliError::new(
                        ErrorKind::Busy,
                        "Stop the daemon before forgetting a board.",
                    )
                    .with_hint("Run ppn daemon stop, then retry board forget.")
                } else {
                    unreachable(error)
                }
            },
        )?;
        let released = app.release_board_relays(board.id)?;
        kept = relays_keeping_a_copy(released.relay_copies(board.id))?;
        app.forget_board(board.id)?;
    }
    let warnings: Vec<_> = kept
        .iter()
        .map(|relay| {
            format!(
                "Relay {relay} keeps its copy of {}. Run ppn relay drop on the relay to delete it.",
                single_line(&board.name)
            )
        })
        .collect();
    Ok(CommandResult::document(ResultDocument {
        human: if confirmed {
            format!("Forgot {} on this device.", single_line(&board.name))
        } else {
            "Forget cancelled.".into()
        },
        fields: Map::from_iter([
            ("kind".into(), json!("board_forget")),
            ("board".into(), json!(board)),
            ("forgotten".into(), json!(confirmed)),
            ("relay_copies_kept".into(), json!(kept)),
        ]),
    })
    .with_warnings(warnings))
}

fn relays_keeping_a_copy(
    copies: impl Iterator<Item = (DeviceId, RelayCopy)>,
) -> Result<Vec<DeviceId>, CliError> {
    let mut kept = Vec::new();
    for (relay, copy) in copies {
        match copy {
            RelayCopy::Absent => (),
            RelayCopy::Kept => kept.push(relay),
            RelayCopy::RemovalPending => {
                return Err(CliError::new(
                    ErrorKind::Busy,
                    "A relay has not yet answered the request to delete its copy of this board.",
                )
                .with_hint(
                    "Run ppn daemon start, wait until ppn board relay ls shows the removal as removed or unreachable, then run ppn daemon stop and retry.",
                ));
            }
        }
    }
    Ok(kept)
}

#[cfg(test)]
#[test]
fn forgetting_waits_for_pending_relay_removals_and_names_the_copies_kept() {
    let [greenhouse_relay, attic_relay, cellar_relay] =
        [31, 47, 59].map(|byte| DeviceId::from_bytes([byte; 32]));
    let settled = [
        (greenhouse_relay, RelayCopy::Absent),
        (attic_relay, RelayCopy::Kept),
        (cellar_relay, RelayCopy::Kept),
    ];
    assert_eq!(
        relays_keeping_a_copy(settled.into_iter()).unwrap(),
        [attic_relay, cellar_relay]
    );
    assert_eq!(relays_keeping_a_copy([].into_iter()).unwrap(), []);
    let waiting = [
        (attic_relay, RelayCopy::Kept),
        (greenhouse_relay, RelayCopy::RemovalPending),
    ];
    let error = relays_keeping_a_copy(waiting.into_iter()).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Busy);
    assert!(error.to_string().contains("has not yet answered"));
}

struct ResultDocument {
    human: String,
    fields: Map<String, Value>,
}

impl Render for ResultDocument {
    fn human(&self, _: &Terminal) -> String {
        self.human.clone()
    }
    fn json_fields(self) -> Map<String, Value> {
        self.fields
    }
}
