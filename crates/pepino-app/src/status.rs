use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IdentityRecord {
    pub id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NamedRecord {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StatusCounts {
    pub open: usize,
    pub closed: usize,
    pub events: i64,
    pub skipped: SkippedCounts,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkippedCounts {
    pub unknown_version: i64,
    pub unknown_event: i64,
    pub unknown_value: i64,
    pub invalid: i64,
}
