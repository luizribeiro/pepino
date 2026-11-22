use std::collections::BTreeMap;

use ppn::{output::Render, terminal::Terminal};
use serde_json::{Map, Value, json};

const FILTERS: &str = r#"Terms are ANDed. Prefix a term with - to negate it. An unquoted comma
separates OR alternatives in a keyed term. Omit a comparison operator for
equality. Use <, <=, > or >= with priority, due, created, updated and custom
fields. Priority accepts signed 64-bit integers. Custom fields accept
integers, dates or text, in that order. Quote a custom operand to make it text.
Both keys and custom field names use lowercase ASCII letters, digits, underscores
and hyphens, start with a letter and are at most 64 bytes. An unknown key
is an input error. Tags use
lowercase letters, digits, underscores, hyphens, slashes and periods.
Open includes todo and doing. Closed includes done and cancelled.
A blocked task has an open blocker. Ready means todo with no open blockers.
has: also accepts custom field names. assignee:me selects the local identity.
Use ppn help ids for task references used with parent, blocks and blocked-by.
board: accepts a board name or an unambiguous hexadecimal ID prefix.

Free text searches titles, descriptions and comments by case-insensitive
substring. @name expands a saved view. Saved views follow you to all your
devices. The latest change wins. A recovered legacy view appears as
name (from device) [@key] in ppn view ls. Use @key in filters and key
with ppn view show or rm. JSON lists the label as recovered_as and names
that could not be imported in not_imported.
Use ppn help dates for date forms.
Double quotes in filter syntax protect spaces, commas and operators.
Inside quotes, use \" for a literal double quote and \\ for a backslash.
The shell removes its own quotes. Each positional argument is one term.
Whitespace inside a keyed argument stays in its operand. Other arguments
with internal whitespace are literal text. Empty arguments are ignored.
Use --filter (or -f) for a full filter string with spaces and quoted phrases.
It is ANDed with the positional terms. Pass a returned filter via --filter.
Shell-quote terms containing whitespace or < and >. Use ppn ls -- -tag:later
to end option parsing explicitly.

Examples:
  ppn ls tag:greenhouse,hardware -tag:later
  ppn ls status:doing assignee:me
  ppn ready 'due:<=fri' has:description
  ppn ls 'replace the greenhouse latch'
  ppn view save weekend is:open 'due:<=+1w'
  ppn ls --filter '@weekend client:"Acme Corp" estimate:<=3'"#;

const DATES: &str = "Use YYYY-MM-DD with a signed year outside 0000–9999. Relative forms include today, tomorrow, yesterday, a three-letter or full weekday, \
or a signed offset such as +3d or -1w. Units are d (days) and w (weeks). \
A weekday chooses the next occurrence including today. Keywords ignore case. Relative dates use the local calendar.";

const JSON: &str = r#"Use --json or PPN_JSON=1. Success writes one versioned document to stdout
with raw values, full IDs and UTC timestamps. Every document has schema and
kind. Errors and warnings go to stderr under error and warning respectively.
See ppn help --json for command response fields, exit statuses and error codes.

Use ppn add --from-json FILE or --from-json - (stdin) for a task object.
The title is required. Optional keys are description, status, due, priority,
tags, assignees, fields, parent, blocks and blocked_by. An optional op must be
"add_task". Status defaults to todo. Due accepts the forms in ppn help dates.
Priority is a signed 64-bit integer. Tags, assignees, blocks and blocked_by
are string arrays. Parent, blocks and blocked_by use task IDs or prefixes.
Each entry of fields is a typed value such as {"type":"integer","value":3}.
Other value types are text, date, status or unset. Date values may be relative.
Omit value for unset.
Explicit flags override scalar values and append tags, assignees and dependencies.
The --json flag selects output only. ppn add - reads literal title text.

Example:
  {"title":"Replace the greenhouse latch","due":"tomorrow","tags":["hardware"],"fields":{"estimate":{"type":"integer","value":3}}}"#;

pub(super) fn all() -> BTreeMap<&'static str, String> {
    let keywords = pepino_app::filter_keyword_groups()
        .map(|(group, words)| format!("{group} {}", words.join(", ")))
        .join("\n");
    BTreeMap::from([
        ("filters", format!("{keywords}\n\n{DATES}\n\n{FILTERS}")),
        ("dates", DATES.into()),
        ("ids", "Task IDs are 64 hexadecimal digits. Use any unambiguous prefix of at least four digits. Assignees accept me, a display name, a full identity ID or an unambiguous prefix. Display names match case-insensitively.".into()),
        ("json", JSON.into()),
    ])
}

pub(super) struct Topic(pub(super) String, pub(super) String);

impl Render for Topic {
    fn human(&self, _: &Terminal) -> String {
        self.1.clone()
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), json!("help_topic")),
            ("topic".into(), json!(self.0)),
            ("text".into(), json!(self.1)),
        ])
    }
}
