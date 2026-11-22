use std::{path::Path, time::Duration};

use pepino_app::{MUTUAL_REMOVAL_RISK, ManagePerson, RelayCopy};
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
    let wait = context.timeout.unwrap_or(Duration::from_secs(30));
    let risk = {
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
    };
    let confirmed = prompt.map_or(Ok(true), |prompt| context.confirm(&prompt))?;
    if confirmed {
        let daemon = super::request::running(context, config_path, allow_start)?;
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
            Outcome::BoardManaged => (),
            Outcome::Error { code, message } => {
                return Err(super::request::control_error(&code, message));
            }
            _ => return Err(unexpected_reply()),
        }
    }
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
            ("complete".into(), json!(confirmed)),
            ("consequences".into(), json!(notice)),
        ]),
    }))
}

const LOCAL_LEAVE: &str = "Other members still see you listed until a manager removes you.";
const LAST_MANAGER: &str = "The board will be left without a manager. Nobody will be able to invite or remove people. Promote someone first if the board should stay managed.";

pub(super) enum Discard {
    Leave { acknowledge: bool },
    Forget,
}

pub(super) fn discard(
    context: &Context<'_>,
    reference: Option<&str>,
    action: Discard,
) -> Result<CommandResult, CliError> {
    let app = AppClient::open(context)?;
    let local = app.local_state();
    let board = match reference {
        Some(reference) => local.resolve_board(reference, "board")?,
        None => app.selected_board()?.0,
    };
    local.check_invitable(board.id)?;
    let active = !local.board_removed(board.id);
    let last_manager = if active {
        let path = context
            .config()?
            .config
            .data_dir
            .value
            .as_path()
            .join("node.sqlite");
        let members = if path.exists() {
            runtime()?
                .block_on(Node::persisted_members(&path, board.id))
                .map_err(|_| {
                    CliError::new(
                        ErrorKind::Failure,
                        "Could not read the local board membership.",
                    )
                })?
        } else {
            Default::default()
        };
        local.is_only_board_manager(board.id, &members)
    } else {
        false
    };
    if last_manager && context.yes && !matches!(action, Discard::Leave { acknowledge: true }) {
        return Err(CliError::new(ErrorKind::ConfirmationRequired, LAST_MANAGER)
            .with_hint("Pass --yes and --i-understand-last-manager-risk to board leave."));
    }
    let name = single_line(&board.name);
    let mut prompt =
        format!("Delete the local task and membership history for {name} on all your devices?");
    if active {
        prompt.push(' ');
        prompt.push_str(LOCAL_LEAVE);
    }
    if last_manager {
        prompt.push(' ');
        prompt.push_str(LAST_MANAGER);
    }
    let confirmed = context.confirm(&prompt)?;
    let kept: std::collections::BTreeSet<DeviceId> = if confirmed {
        app.forget_board(board.id)?
            .relay_copies(board.id)
            .filter_map(|(relay, copy)| (copy == RelayCopy::Kept).then_some(relay))
            .collect::<std::collections::BTreeSet<_>>()
    } else {
        Default::default()
    };
    let leaving = matches!(action, Discard::Leave { .. });
    let mut fields = Map::from_iter([
        (
            "kind".into(),
            json!(if leaving {
                "board_leave"
            } else {
                "board_forget"
            }),
        ),
        ("board".into(), json!(board)),
        ("relay_copies_kept".into(), json!(kept)),
    ]);
    fields.insert(
        if leaving { "left" } else { "forgotten" }.into(),
        json!(confirmed),
    );
    let still_listed = confirmed && active;
    fields.insert("still_listed_as_member".into(), json!(still_listed));
    let warnings = kept.into_iter().map(|relay| format!(
        "Relay {relay} may keep its copy of {name}. Run ppn relay drop on the relay to delete it."
    )).collect::<Vec<_>>();
    Ok(CommandResult::document(ResultDocument {
        human: match (confirmed, leaving, still_listed) {
            (true, _, true) => format!("Deleted {name} from your local catalog. {LOCAL_LEAVE}"),
            (true, _, false) => format!("Forgot {name} from your local catalog."),
            (false, true, _) => "Leave cancelled.".into(),
            (false, false, _) => "Forget cancelled.".into(),
        },
        fields,
    })
    .with_warnings(warnings))
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
