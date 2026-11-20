use std::collections::BTreeMap;

use ppn::{output::Render, terminal::Terminal};
use serde_json::{Map, Value, json};

const FILTERS: &str = r#"Terms are ANDed. Prefix a term with - to negate it. An unquoted comma
separates OR alternatives in a keyed term. Omit a comparison operator for
equality. Use <, <=, > or >= with priority, due, created, updated and custom
fields. Priority accepts signed 64-bit integers. Custom fields accept
integers, dates or text, in that order. Quote a custom operand to make it text.
Both keys and custom field names use lowercase letters, digits, underscores
and hyphens, start with a letter and are at most 64 bytes. The tags use
lowercase letters, digits, underscores, hyphens, slashes and periods.
has: also accepts custom field names. assignee:me selects the local identity.
Use ppn help ids for task references used with parent, blocks and blocked-by.
board: accepts a board name or full ID.

Free text searches titles, descriptions and comments by case-insensitive
substring. @name expands a saved view. Use ppn help dates for date forms.
Double quotes in filter syntax protect spaces, commas and operators.
Inside quotes, use \" for a literal double quote and \\ for a backslash.
The shell removes its own quotes. Each positional argument is one term.
Shell-quote terms containing whitespace or < and >. Use ppn ls -- -tag:later
to end option parsing explicitly.

Examples:
  ppn ls tag:greenhouse,hardware -tag:later
  ppn ls status:doing assignee:me
  ppn ready 'due:<=fri' has:description
  ppn ls 'replace the greenhouse latch'
  ppn view save weekend is:open 'due:<=+1w'
  ppn ls @weekend 'client:"Acme Corp"' 'estimate:<=3'"#;

const DATES: &str = "Use YYYY-MM-DD, today, tomorrow, yesterday, a three-letter or full weekday, \
or a signed offset such as +3d or -1w. Units are d (days) and w (weeks). \
A weekday chooses the next occurrence including today. Relative dates use the local calendar.";

pub(super) fn all() -> BTreeMap<&'static str, String> {
    let keywords = pepino_app::filter_keyword_groups()
        .map(|(group, words)| format!("{group} {}", words.join(", ")))
        .join("\n");
    BTreeMap::from([
        ("filters", format!("{keywords}\n\n{DATES}\n\n{FILTERS}")),
        ("dates", DATES.into()),
        ("ids", "Task IDs are 64 hexadecimal digits. Use any unambiguous prefix of at least four digits. Assignees accept me, a display name, a full identity ID or an unambiguous prefix.".into()),
        ("json", "Use --json or PPN_JSON=1. Success writes one versioned document to stdout with raw values, full IDs and UTC timestamps. Errors and warnings go to stderr as error or warning objects. See ppn help --json for exit statuses and error codes.".into()),
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
