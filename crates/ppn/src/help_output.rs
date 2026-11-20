use clap::Command;
use serde_json::{Value, json};

pub(super) fn for_command(command: &Command) -> Value {
    let name = command.get_bin_name().unwrap_or_else(|| command.get_name());
    let name = name.strip_prefix("ppn ").unwrap_or(name);
    let (kind, fields) = match name {
        "add" | "edit" | "start" | "reopen" | "done" | "cancel" | "rm" | "tag" | "untag"
        | "assign" | "unassign" | "comment" | "link" | "unlink" => (
            "write",
            "changed Whether any change was recorded\ncreated Full ID of the task created by add\ntasks Current records affected by the write\ntargets IDs directly addressed by the command\ndeleted Snapshots of deleted tasks\nedges Results for affected relationships\nactivated Previously excluded relationships that became effective\ntop_level Tasks that became top-level after deletion\nunblocked Tasks unblocked by deletion\nnames Display names keyed by identity or device ID",
        ),
        "ls" | "ready" => (
            "tasks",
            "tasks Matching task records after sorting and limiting, with board context for listings across boards\ntotal Number of matches before the limit\nfilter Canonical filter accepted by --filter\nresolved_dates Calendar dates for relative operands\nsort Sort specification used\nwarnings Filter warnings\nconstrains_status Whether the user filter selects a status\nnames Display names keyed by identity or device ID",
        ),
        "show" => (
            "task",
            "task Task details including its ID and comments\nedges All incident relationships with their effective flags\nrelation_titles Related task titles by ID\nassignee_names Assignee display names by ID\nnames Display names keyed by identity or device ID",
        ),
        "tree" => (
            "tree",
            "nodes Task and descendant records with depths\nnames Display names keyed by identity or device ID",
        ),
        "deps" => (
            "deps",
            "task Addressed task\nblockers Upstream dependencies with distances\ndependents Downstream dependencies with distances\nnames Display names keyed by identity or device ID",
        ),
        "log" => (
            "log",
            "titles Task titles by ID\nevents Ordered events\nunordered Events without a readable clock\nnames Display names keyed by identity or device ID",
        ),
        "init" | "status" => (
            "status",
            "identity Identity name, shared roster, cache status and rejected roster entries\ndevice Local device, known revocation or pending leave, and last peer contact when available\nboard Selected board\nselected_board Board selected for this invocation and its source\nboards Known boards with membership and sync activity\ndata_dir Local data directory\ncounts Task and event counts\nnetwork Daemon network snapshot or offline\nprotocol_version Application protocol version",
        ),
        "board request" => (
            "board_request",
            "request Secret board request code\nfingerprint Fingerprint to confirm with the invitee\ndevice_id Requesting device ID\nexpires_at Request expiry in UTC\nsecret True for secret code output",
        ),
        "board invite" => (
            "board_grant",
            "grant Secret board grant code\nfingerprint Confirmed request fingerprint\ndevice_id Invited device ID",
        ),
        "board join" => (
            "board_join",
            "joined Whether the board finished its first sync",
        ),
        "board ls" => (
            "boards",
            "boards Known boards with current marker, counts, membership and sync activity\nselected_board Board selected for this invocation and its source",
        ),
        "board show" => (
            "board",
            "board Board with current marker, counts, membership and sync activity\nselected_board Board selected for this invocation and its source",
        ),
        "board new" | "board use" | "board rename" => (
            "board",
            "board Affected board with counts, membership and sync activity",
        ),
        "daemon run" | "daemon start" | "daemon stop" | "daemon status" => (
            "daemon",
            "action Lifecycle operation performed\nrunning Whether the daemon is running\npid Running process ID or null\nuptime_ms Monotonic uptime in milliseconds or null\nsocket_path Effective control socket path",
        ),
        "sync" => (
            "sync",
            "idle Whether all known sync work settled\nnetwork Peers and per-board sync activity\nsynced_devices IDs and known names of devices with a successful board sync",
        ),
        "identity show" | "identity rename" => (
            "identity",
            "identity Identity ID, display name, shared roster, cache status and rejected roster entry count",
        ),
        "device request" => (
            "device_request",
            "request Secret request code for an existing device\nsecret Whether the request code is secret\ndevice_id Requesting device ID\nfingerprint Fingerprint to verify on both hosts\nexpires_at Expiry of the signed member information",
        ),
        "device add" => (
            "device_grant",
            "grant Secret object with code and secret fields\ndevice_id Enrolled device ID\nfingerprint Fingerprint of the confirmed request",
        ),
        "device join" => (
            "device_join",
            "joined Whether every board finished its first sync\nboards Per-board join states\ngrantor Signing device ID and name\nmessage Null on success",
        ),
        "device revoke" => (
            "device_revoke",
            "device_id Full revoked device ID\nname Device name\nconfirmed Whether revocation was confirmed\nboards Per-board removal outcomes",
        ),
        "device leave" => (
            "device_leave",
            "device_id Full ID of the retiring device\nrequested_at Request time in UTC or null\npending Whether retirement is pending\ndaemon_notified Whether the local daemon was notified\ndata_dir Local data directory to wipe\ncredentials_file Local credentials file to wipe",
        ),
        "device rename" => (
            "roster_device",
            "device Renamed device with its ID, name, state and admission metadata",
        ),
        "device ls" => (
            "devices",
            "devices Local device observations including board counts and last seen time\nroster Shared roster with device IDs, names, admission metadata, revocations and pending leave requests",
        ),
        "view ls" => ("views", "views Saved filters ordered by name"),
        "view save" => (
            "view_saved",
            "view Saved name and filter\nreplaced Whether a previous definition was replaced",
        ),
        "view rm" => ("view_deleted", "view Removed name and filter"),
        "view show" => ("view", "view Saved name and canonical filter"),
        "config show" => (
            "config",
            "values Effective values and their sources\nfiles Configuration files considered",
        ),
        "config path" => ("config_paths", "files Configuration paths and existence"),
        "config check" => (
            "config_check",
            "valid Whether configuration is valid\nfiles Configuration paths and existence",
        ),
        "version" => ("version", "version Installed ppn version"),
        "ppn" | "help" => (
            "help",
            "name Command name\nsummary Command summary\naliases Alternative command names\narguments Positional argument descriptions\nflags Available options and values\ncommands Child commands\nexamples Example invocations\noutput Success document description\nenvironment Configuration environment variables when applicable\nexit_codes Exit statuses\nerror_codes Stable error codes\ntopics Help topic text by name",
        ),
        "debug confirm" => ("confirmation", "confirmed Whether the user confirmed"),
        "debug input" => ("text", "text Text read from the chosen input"),
        _ => return Value::Null,
    };
    let mut document = shape(kind, fields);
    if name == "device request" {
        document["cancel_output"] = shape(
            "device_request_cancel",
            "cancelled Whether a pending request was removed",
        );
    }
    if name == "help" {
        document["topic_output"] =
            shape("help_topic", "topic Requested topic name\ntext Topic text");
    }
    document
}

fn shape(kind: &str, fields: &str) -> Value {
    let common = [
        ("schema", "CLI output schema version"),
        ("kind", "Document type"),
    ];
    let fields: serde_json::Map<String, Value> = common
        .into_iter()
        .chain(fields.lines().filter_map(|field| field.split_once(' ')))
        .map(|(name, meaning)| (name.to_owned(), meaning.into()))
        .collect();
    json!({"kind": kind, "fields": fields})
}
