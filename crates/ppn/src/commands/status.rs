use pepino_app::{OpenOptions, ReadRequest, Request};
use ppn::app::AppClient;
use ppn::daemon::{
    address::socket_path,
    client::Client,
    network::membership,
    protocol::{Board, NEAR_MEMBER_LIMIT_NOTICE, Network},
};
use ppn::error::{CliError, shorten_message_ids};
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::{plural, short_id, single_line};
use ppn::response::{ResponseExt, Status};
use ppn::terminal::{Context, RenderSettings};

#[derive(clap::Args)]
/// Show local identity, board, task counts, and sync activity.
#[command(after_help = example_help(&["ppn status", "ppn status --json"]))]
pub struct Args {}

pub fn run(context: &Context<'_>, _: Args) -> Result<CommandResult, CliError> {
    if pepino_app::is_relay(context.config()?.config.data_dir.value.as_path())? {
        return super::relay::status(context);
    }
    let path = socket_path(&context.config()?.config);
    let network = (Client { path })
        .status()
        .ok()
        .flatten()
        .map(|status| status.network);
    if let Some(network) = network.as_ref()
        && let Some(elapsed) = network.starting_ms
    {
        return Ok(CommandResult::document(Starting(elapsed)));
    }
    document(context, AppClient::open(context)?, network, None)
}

struct Starting(u64);

impl ppn::output::Render for Starting {
    fn human(&self, _: &ppn::terminal::Terminal) -> String {
        ppn::daemon::report::describe_startup(self.0)
    }

    fn json_fields(self) -> serde_json::Map<String, serde_json::Value> {
        use serde_json::json;
        serde_json::Map::from_iter([
            ("kind".into(), json!("status")),
            ("network".into(), json!({"starting_ms": self.0})),
        ])
    }
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
    let client = AppClient::init(context, options)?;
    let initialized = client.initialized();
    document(context, client, None, Some(initialized))
}

fn document(
    context: &Context<'_>,
    mut client: AppClient,
    network: Option<Network>,
    initialized: Option<bool>,
) -> Result<CommandResult, CliError> {
    let response = client.call(Request::Read(ReadRequest::Status))?;
    let local = client.local_state();
    let revocation = local.device_revocation().cloned();
    let leave_requested_at = local.device_leave_requested_at();
    let notice = local
        .check_device_write()
        .err()
        .map(|error| {
            format!(
                "\n{}",
                shorten_message_ids(&single_line(
                    CliError::from(error).localize(context).human_message()
                ))
            )
        })
        .unwrap_or_default();
    let newer_catalog = local.newer_catalog_version();
    let upgrade = newer_catalog.map_or("", |_| pepino_app::CATALOG_UPGRADE_NOTICE);
    let upgrade_line = if upgrade.is_empty() {
        String::new()
    } else {
        format!("\n{upgrade}")
    };
    let roster = local.roster();
    let last_contact = roster
        .keys()
        .filter(|&&id| id != local.device_id)
        .filter_map(|&id| {
            let seen = network.as_ref().and_then(|network| {
                let id = id.to_string();
                network
                    .peers_synced
                    .contains(&id)
                    .then(|| network.last_seen.get(&id).copied())
                    .flatten()
            });
            seen.max(local.observed_last_seen(id))
        })
        .max();
    let no_contact_days = last_contact.map(|time| {
        let now =
            chrono::DateTime::from_timestamp_millis(context.time.now_ms as i64).unwrap_or(time);
        (now - time).num_days().max(0)
    });
    let contact = match no_contact_days {
        Some(days @ 1..) => format!(
            "\nNo contact with any other device for {days} {}",
            plural(days, "day", "days")
        ),
        None if roster.len() > 1 => "\nNo contact with any other device yet".into(),
        _ => String::new(),
    };
    let roster_stale = !local.roster_ready()
        || (roster.len() > 1
            && network.as_ref().is_none_or(|net| {
                net.peers_synced.is_empty()
                    || net
                        .boards
                        .get(&local.catalog_board().to_string())
                        .is_none_or(|board| {
                            board.starting_up || board.active || board.last_error.is_some()
                        })
            }));
    let roster_note = if roster_stale {
        if network.is_some() {
            " · roster cache may be stale (catalog sync pending)"
        } else {
            " · roster cache may be stale (daemon offline)"
        }
    } else {
        ""
    };
    let rejected_board_card_entries = local.rejected_board_cards();
    let dropped_board_cards = local.dropped_board_cards();
    let dropped_membership_records = local.dropped_membership_records();
    let boards = local
        .boards()
        .iter()
        .map(|board| {
            let (state, sync) = super::board::board_activity(
                board.id,
                local.board_join_state(board.id),
                network.as_ref(),
            );
            let access = local.shared_access(board.id);
            let waiting = local.waiting_for_access(board.id);
            let owner = access.map(|access| pepino_app::NamedRecord {
                id: access.owner.to_string(),
                name: local.board_owner_name(board.id, access),
            });
            (
                board.clone(),
                state,
                sync,
                access.map_or(pepino_app::BoardRole::Manage, |access| access.role),
                owner,
                waiting,
                local.board_departure(board.id),
            )
        })
        .collect::<Vec<_>>();
    let membership_rows = boards
        .iter()
        .map(|(board, state, sync, role, owner, waiting, departure)| {
            let departure = departure.map(|state| state.to_string());
            let state = if let Some(departure) = departure.as_deref() {
                departure
            } else if *waiting {
                "waiting for access"
            } else {
                membership(*state)
            };
            let sharing = owner.as_ref().map_or_else(String::new, |owner| {
                format!(
                    " · shared by {}",
                    if owner.name.is_empty() {
                        short_id(&owner.id).to_owned()
                    } else {
                        single_line(&owner.name)
                    }
                )
            });
            format!(
                "{}: {state} ({}) · {role}{sharing}",
                single_line(&board.name),
                if departure.is_some() {
                    "paused".into()
                } else {
                    sync.replace('_', " ")
                }
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
            if let Some(initialized) = initialized {
                return Ok(if initialized {
                    format!(
                        "Initialised pepino.\nIdentity: {}\nDevice: {}\nBoard: {}\nNext, run ppn add to create a task.",
                        single_line(&identity.display_name),
                        single_line(&device.name),
                        single_line(&board.name)
                    )
                } else {
                    "Pepino is already initialised.".into()
                });
            }
            let skipped = &counts.skipped;
            let network = human_network
                .as_deref()
                .unwrap_or("offline (daemon not running)");
            let device_count = identity.roster.len();
            let device_noun = plural(device_count, "device", "devices");
            Ok(format!(
                "Identity: {} ({}) · {device_count} {device_noun}{roster_note}\nRejected roster entries: {}\nRejected board card entries: {rejected_board_card_entries}\nDropped board cards: {dropped_board_cards}\nDropped refused membership records: {dropped_membership_records}\nDevice: {} (node ID {}){notice}{contact}\nBoard: {} ({})\n{selected_line}\nMembership:\n  {membership_rows}\nData directory: {}\nCredentials: {}\nnetwork: {network}\nTasks: {} open, {} closed\nEvents: {} total, {} applied\nSkipped events: {} unknown version, {} unknown event, {} unknown value, {} invalid, {} forged author\nProtocol: v{}{upgrade_line}",
                single_line(&identity.display_name),
                short_id(&identity.id),
                identity.rejected_roster_entries,
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
            if let Some(version) = newer_catalog {
                fields.insert(
                    "newer_catalog".into(),
                    serde_json::json!({"version":version,"message":upgrade}),
                );
            }
            let boards = boards.into_iter().map(|(board, state, sync, role, owner, waiting, departure)| {
                let membership = if let Some(departure) = departure { serde_json::json!(departure) } else if waiting { serde_json::json!("waiting_for_access") } else { serde_json::json!(state) };
                serde_json::json!({"id": board.id, "name": board.name, "membership": membership, "sync": if departure.is_some() { "paused" } else { sync }, "role": role, "shared": owner.is_some(), "owner": owner})
            }).collect::<Vec<_>>();
            fields.insert(
                "rejected_board_card_entries".into(),
                serde_json::json!(rejected_board_card_entries),
            );
            fields.insert(
                "dropped_membership_records".into(),
                serde_json::json!(dropped_membership_records),
            );
            fields.insert("boards".into(), serde_json::json!(boards));
            fields.insert("selected_board".into(), selected);
            if let Some(identity) = fields.get_mut("identity") {
                identity["cache_stale"] = serde_json::json!(roster_stale);
            }
            if let Some(device) = fields.get_mut("device") {
                if let Some(revocation) = revocation {
                    device["revocation"] = serde_json::json!(revocation);
                }
                if let Some(at) = leave_requested_at {
                    device["leave_requested_at"] = serde_json::json!(at);
                }
                if roster.len() > 1 {
                    device["last_contact"] = serde_json::json!(last_contact);
                    device["no_contact_days"] = serde_json::json!(no_contact_days);
                }
            }
            if let Some(network) = network {
                fields.insert("network".into(), serde_json::json!(network));
            }
        },
    )
}

fn describe_network(network: &Network, settings: &RenderSettings) -> String {
    use std::fmt::Write;

    let mut text = format!(
        "{} {} seen, {} connected",
        network.peers_seen.len(),
        plural(network.peers_seen.len(), "peer", "peers"),
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
        let events = plural(board.events_waiting.unwrap_or_default(), "event", "events");
        let _ = write!(
            text,
            "\n  Board {}: {starting}{pending} {events} waiting to be sent, last sent {}, last received {}",
            short_id(id),
            time(board.last_sent),
            time(board.last_received)
        );
        text.push_str(&describe_board_problems(board));
    }
    text
}

fn describe_board_problems(board: &Board) -> String {
    let mut text = String::new();
    if let Some(error) = &board.last_error {
        text = format!(", Last error: {}", describe_sync_error(error));
    }
    if let Some(error) = &board.read_error {
        text.push_str(&format!(", Read error: {}", describe_sync_error(error)));
    }
    if board.near_member_limit {
        text.push_str("\n    Warning: ");
        text.push_str(NEAR_MEMBER_LIMIT_NOTICE);
    }
    text
}

#[cfg(test)]
#[test]
fn board_problems_list_the_last_error_then_the_member_limit_warning() {
    let board = |last_error: Option<&str>, near_member_limit| Board {
        read_error: None,
        starting_up: false,
        enrollment_pending: false,
        active: false,
        events_waiting: Some(0),
        published: 12,
        received: 31,
        last_sent: None,
        last_received: None,
        last_error: last_error.map(Into::into),
        near_member_limit,
    };
    assert_eq!(describe_board_problems(&board(None, false)), "");
    assert_eq!(
        describe_board_problems(&board(None, true)),
        "\n    Warning: this board is approaching the size at which new members can no longer be added. See \"Boards with many removals\" in docs/sharing.md."
    );
    assert_eq!(
        describe_board_problems(&board(
            Some("publish 0303: the operation was not submitted"),
            true
        )),
        format!(
            ", Last error: Could not publish 0303: the operation was not submitted.\n    Warning: {NEAR_MEMBER_LIMIT_NOTICE}"
        )
    );
    let unreadable = Board {
        read_error: Some(
            "read a board operation during sync: failed decoding incoming header bytes: LengthTooLarge".into(),
        ),
        ..board(Some("publish 0303: the operation was not submitted"), false)
    };
    assert_eq!(
        describe_board_problems(&unreadable),
        ", Last error: Could not publish 0303: the operation was not submitted., Read error: Could not read a board operation during sync: failed decoding incoming header bytes: LengthTooLarge."
    );
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
