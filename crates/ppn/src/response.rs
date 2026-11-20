use std::collections::BTreeMap;

use pepino_app::{
    Date, DependencyRecord, EdgeEffect, EventRecord, IdentityRecord, NamedRecord, Response,
    SavedView, StatusCounts, TaskRecord, TaskSummary, TreeNode,
};

use crate::error::{CliError, ErrorKind};

macro_rules! response_views {
    ($($method:ident => $kind:ident { $($field:ident: $ty:ty),+ $(,)? }),+ $(,)?) => {
        $(
            pub struct $kind<'a> { $(pub $field: $ty),+ }
        )+

        pub trait ResponseExt {
            $(fn $method(&self) -> Result<$kind<'_>, CliError>;)+
        }

        impl ResponseExt for Response {
            $(
                fn $method(&self) -> Result<$kind<'_>, CliError> {
                    match self {
                        Self::$kind { $($field),+, .. } => Ok($kind { $($field),+ }),
                        _ => Err(unexpected(stringify!($method))),
                    }
                }
            )+
        }
    };
}

type Names = BTreeMap<String, String>;

response_views! {
    write => Write {
        changed: &'a bool, created: &'a Option<String>, tasks: &'a [TaskSummary],
        edges: &'a [EdgeEffect], names: &'a Names, deleted: &'a [TaskSummary],
        targets: &'a [String],
    },
    task => Task {
        task: &'a TaskRecord, edges: &'a [EdgeEffect], relation_titles: &'a Names,
        names: &'a Names, assignee_names: &'a Names,
    },
    tasks => Tasks {
        tasks: &'a [TaskSummary], names: &'a Names, total: &'a usize, filter: &'a str,
        resolved_dates: &'a BTreeMap<String, Date>, sort: &'a str,
    },
    tree => Tree { nodes: &'a [TreeNode], names: &'a Names },
    deps => Deps {
        task: &'a TaskSummary, blockers: &'a [DependencyRecord],
        dependents: &'a [DependencyRecord], names: &'a Names,
    },
    log => Log {
        titles: &'a BTreeMap<String, Option<String>>, events: &'a [EventRecord],
        unordered: &'a [EventRecord], names: &'a Names,
    },
    status => Status {
        identity: &'a IdentityRecord, device: &'a NamedRecord, board: &'a NamedRecord,
        data_dir: &'a str, counts: &'a StatusCounts, protocol_version: &'a u32,
    },
    view_saved => ViewSaved { view: &'a SavedView, replaced: &'a bool },
    view_deleted => ViewDeleted { view: &'a SavedView },
    views => Views { views: &'a [SavedView] },
}

fn unexpected(kind: &str) -> CliError {
    CliError::new(
        ErrorKind::Failure,
        format!("Expected the app to return {kind}."),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_typed_rows_and_reports_an_unexpected_response() {
        let response: Response = serde_json::from_value(serde_json::json!({
            "kind":"tasks","tasks":[],"total":0,"filter":"tag:hardware","sort":"title","resolved_dates":{}
        })).unwrap();
        assert_eq!(response.tasks().unwrap().filter, "tag:hardware");
        let error = response.task().err().unwrap();
        assert_eq!(error.kind(), ErrorKind::Failure);
        assert_eq!(error.to_string(), "Expected the app to return task.");
    }
}
