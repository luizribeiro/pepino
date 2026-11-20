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
            "tasks Matching task records after sorting and limiting\ntotal Number of matches before the limit\nfilter Canonical filter accepted by --filter\nresolved_dates Calendar dates for relative operands\nsort Sort specification used\nwarnings Filter warnings\nconstrains_status Whether the user filter selects a status\nnames Display names keyed by identity or device ID",
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
            "identity Local person\ndevice Local device\nboard Current board\ndata_dir Local data directory\ncounts Task and event counts\nnetwork Daemon network snapshot or offline\nprotocol_version Application protocol version",
        ),
        "daemon run" | "daemon start" | "daemon stop" | "daemon status" => (
            "daemon",
            "action Lifecycle operation performed\nrunning Whether the daemon is running\npid Running process ID or null\nuptime_ms Monotonic uptime in milliseconds or null\nsocket_path Effective control socket path",
        ),
        "sync" => (
            "sync",
            "idle Whether all known sync work settled\nnetwork Peers and per-board sync activity",
        ),
        "device request" => (
            "device_request",
            "request Reusable request code for an existing device",
        ),
        "device add" => (
            "device_grant",
            "grant Secret object with code and secret fields",
        ),
        "device join" => (
            "device_join",
            "joined Whether every board finished its first sync\nboards Per-board join states\nmessage Recovery guidance after timeout or null",
        ),
        "device ls" => (
            "devices",
            "devices Device IDs, names, current-device marker, state and last seen time",
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
