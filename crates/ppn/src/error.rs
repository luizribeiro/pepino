use std::io::{self, Write};
use std::process::ExitCode;

use pepino_app::{AppError, AppErrorKind};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::render::ShortIds;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum ErrorKind {
    Failure,
    Usage,
    NotFound,
    NotInitialized,
    AmbiguousId,
    PermissionDenied,
    DaemonUnreachable,
    ConfirmationRequired,
    InvalidConfig,
    InvalidInput,
    Conflict,
    Busy,
}

impl ErrorKind {
    pub fn all() -> &'static [Self] {
        <Self as clap::ValueEnum>::value_variants()
    }

    pub const fn meaning(self) -> &'static str {
        match self {
            Self::Failure => "The command failed.",
            Self::Usage => "The arguments or options are invalid.",
            Self::NotFound => "The requested item could not be found.",
            Self::NotInitialized => "Run ppn init to create the local identity, device, and board.",
            Self::AmbiguousId => "The identifier matches multiple items.",
            Self::PermissionDenied => "The operation is not permitted.",
            Self::DaemonUnreachable => "The daemon or network could not be reached.",
            Self::ConfirmationRequired => "The operation needs explicit confirmation.",
            Self::InvalidConfig => "Configuration could not be read or is invalid.",
            Self::InvalidInput => "The request input is invalid.",
            Self::Conflict => "The request conflicts with existing data.",
            Self::Busy => "The data directory is locked by another pepino process.",
        }
    }

    pub const fn status(self) -> u8 {
        match self {
            Self::Failure
            | Self::ConfirmationRequired
            | Self::InvalidConfig
            | Self::Conflict
            | Self::Busy
            | Self::NotInitialized => 1,
            Self::Usage | Self::InvalidInput => 2,
            Self::NotFound => 3,
            Self::AmbiguousId => 4,
            Self::PermissionDenied => 5,
            Self::DaemonUnreachable => 6,
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Self::Failure => "failure",
            Self::Usage => "usage",
            Self::NotFound => "not_found",
            Self::NotInitialized => "not_initialized",
            Self::AmbiguousId => "ambiguous_id",
            Self::PermissionDenied => "permission_denied",
            Self::DaemonUnreachable => "daemon_unreachable",
            Self::ConfirmationRequired => "confirmation_required",
            Self::InvalidConfig => "invalid_config",
            Self::InvalidInput => "invalid_input",
            Self::Conflict => "conflict",
            Self::Busy => "busy",
        }
    }
}

#[derive(Clone, Debug, Serialize, thiserror::Error)]
#[error("{message}")]
pub struct CliError {
    #[serde(skip)]
    kind: ErrorKind,
    code: &'static str,
    message: String,
    hint: Option<String>,
    #[serde(flatten)]
    details: Map<String, Value>,
}

impl CliError {
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            code: kind.code(),
            message: message.into(),
            hint: None,
            details: Map::new(),
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.details
            .insert("field".into(), Value::String(field.into()));
        self
    }

    pub fn usage(error: clap::Error) -> Self {
        let rendered = error.to_string();
        let rendered = rendered.trim();
        let (message, details) = rendered.split_once("\n\n").unwrap_or((rendered, ""));
        let mut error = Self::new(ErrorKind::Usage, message.trim_start_matches("error: "));
        if let Some(tip) = details
            .lines()
            .find_map(|line| line.trim().strip_prefix("tip: "))
        {
            error = error.with_hint(tip);
        }
        error
    }

    pub fn report(&self, json: bool, mut stderr: impl Write) -> ExitCode {
        let _ = self.write(json, &mut stderr);
        self.kind.status().into()
    }

    fn write(&self, json: bool, stderr: &mut dyn Write) -> io::Result<()> {
        if json {
            serde_json::to_writer(
                &mut *stderr,
                &serde_json::json!({ "schema": crate::output::SCHEMA_VERSION, "kind": "error", "error": self }),
            )?;
            return writeln!(stderr);
        }

        let candidates: Vec<_> = self
            .details
            .get("candidates")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let term = self.details.get("term").and_then(Value::as_str);
        let fragments = std::iter::once(self.message.as_str())
            .chain(self.hint.as_deref())
            .chain(term)
            .chain(candidates.iter().copied());
        let ids = ShortIds::new(fragments.flat_map(hex_ids).map(|(_, id)| id));
        writeln!(stderr, "error: {}", display_ids(&self.message, &ids))?;
        for id in candidates {
            writeln!(stderr, "  {}", display_ids(id, &ids))?;
        }
        if let Some(term) = term {
            writeln!(stderr, "  term {:?}", display_ids(term, &ids))?;
        }
        if let Some(position) = self.details.get("position")
            && let Some(byte) = position.get("byte").and_then(Value::as_u64)
        {
            if let Some(index) = position.get("index").and_then(Value::as_u64) {
                writeln!(stderr, "  at argument {index}, byte {byte}")?;
            } else {
                writeln!(stderr, "  at byte {byte}")?;
            }
        }
        if let Some(hint) = &self.hint {
            writeln!(stderr, "hint: {}", display_ids(hint, &ids))?;
        }
        Ok(())
    }
}

fn hex_ids(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut start = 0;
    text.char_indices()
        .chain([(text.len(), ' ')])
        .filter_map(move |(end, ch)| {
            if ch.is_ascii_hexdigit() {
                return None;
            }
            let id = (end - start == 64).then(|| (start, &text[start..end]));
            start = end + ch.len_utf8();
            id
        })
}

fn display_ids(text: &str, ids: &ShortIds<'_>) -> String {
    let mut rendered = String::with_capacity(text.len());
    let mut offset = 0;
    for (start, id) in hex_ids(text) {
        rendered.push_str(&text[offset..start]);
        rendered.push_str(ids.get(id));
        offset = start + id.len();
    }
    rendered.push_str(&text[offset..]);
    rendered
}

impl From<AppError> for CliError {
    fn from(error: AppError) -> Self {
        let kind = match error.kind() {
            AppErrorKind::NotFound => ErrorKind::NotFound,
            AppErrorKind::NotInitialized => ErrorKind::NotInitialized,
            AppErrorKind::PermissionDenied => ErrorKind::PermissionDenied,
            AppErrorKind::AmbiguousId => ErrorKind::AmbiguousId,
            AppErrorKind::InvalidInput => ErrorKind::InvalidInput,
            AppErrorKind::Conflict => ErrorKind::Conflict,
            AppErrorKind::Busy => ErrorKind::Busy,
            _ => ErrorKind::Failure,
        };
        let mut diagnostic = Self::new(kind, error.to_string());
        match &error {
            AppError::NotInitialized { directory } => {
                diagnostic.message = format!("Data directory {directory} is not initialized.");
                diagnostic.hint = Some("Run `ppn init`.".into());
            }
            AppError::NotFound {
                reference: pepino_app::MissingReference::Task(task),
                ..
            } => {
                diagnostic.message = format!("No task matches {task}");
                diagnostic.hint = Some("Run `ppn ls` to list tasks.".into());
            }
            AppError::NotFound {
                reference: pepino_app::MissingReference::DeletedTask { task, title },
                ..
            }
            | AppError::Conflict {
                reference: pepino_app::ConflictReference::Task { task, title },
                ..
            } => {
                let title = title
                    .as_ref()
                    .map_or_else(String::new, |title| format!(" {title:?}"));
                diagnostic.message = format!("Task {task}{title} was deleted");
            }
            AppError::NotFound { reference, .. } => {
                diagnostic.message = format!("{reference} was not found");
            }
            AppError::AmbiguousId { task, count, .. } => {
                diagnostic.message = format!("Task reference {task} matches {count} tasks");
                diagnostic.hint = Some("Use a longer task ID.".into());
            }
            AppError::Conflict {
                reference: pepino_app::ConflictReference::Relation { reason },
                ..
            } => {
                diagnostic.message = reason.clone();
            }
            AppError::Conflict {
                reference: reference @ pepino_app::ConflictReference::View { .. },
                ..
            } => {
                diagnostic.message = reference.to_string();
                diagnostic.hint = Some("Pass `--force` to remove the referenced view.".into());
            }
            AppError::InvalidInput {
                field,
                reason,
                diagnostic: filter,
            } => {
                if reason.starts_with("invalid ") {
                    diagnostic.message = reason.clone();
                }
                diagnostic.hint = if filter.is_some() {
                    Some("Run `ppn ls --help` for filter examples.".into())
                } else {
                    input_hint(field, reason).map(str::to_owned)
                };
            }
            _ => {}
        }
        if let Ok(Value::Object(mut details)) = serde_json::to_value(error) {
            details.remove("kind");
            if let Some(Value::Object(task)) = details.remove("deleted_task") {
                details.extend(task);
            }
            if let Some(Value::String(hint)) = details.remove("hint") {
                diagnostic.hint = Some(hint);
            }
            diagnostic.details = details;
        }
        diagnostic
    }
}

fn input_hint(field: &str, reason: &str) -> Option<&'static str> {
    if field.starts_with("tags[") {
        Some("Use a tag matching [a-z][a-z0-9_./-]* with at most 64 bytes.")
    } else if field.starts_with("assignees[") {
        Some("Use `me`, a display name, or an identity ID or prefix.")
    } else if reason.starts_with("invalid task reference ") {
        Some("Use an id shown by `ppn ls`, at least 4 hex digits.")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_diagnostics_keep_locations_and_hints() {
        let error = serde_json::from_value::<AppError>(serde_json::json!({
            "kind": "invalid_input", "field": "filter", "reason": "unknown filter key",
            "term": "tags:hardware", "position": {"kind": "byte", "byte": 2},
            "hint": "Use tag"
        }))
        .unwrap();
        let mut buffer = Vec::new();
        let error = CliError::from(error);
        error.write(false, &mut buffer).unwrap();
        assert!(
            String::from_utf8(buffer)
                .unwrap()
                .contains("term \"tags:hardware\"\n  at byte 2\nhint: Use tag")
        );
        let mut buffer = Vec::new();
        assert_eq!(error.report(true, &mut buffer), ExitCode::from(2));
        let body: Value = serde_json::from_slice(&buffer).unwrap();
        assert_eq!(body["error"]["code"], "invalid_input");
        assert_eq!(body["error"]["field"], "filter");
        assert_eq!(body["error"]["position"]["byte"], 2);
        assert_eq!(body["error"]["hint"], "Use tag");
    }

    #[test]
    fn filter_errors_without_an_app_hint_suggest_filter_help() {
        let error = serde_json::from_value::<AppError>(serde_json::json!({
            "kind": "invalid_input", "field": "filter", "reason": "empty value",
            "term": "tag:", "position": {"kind": "argument", "index": 1, "byte": 4},
        }))
        .unwrap();
        let mut stderr = Vec::new();
        assert_eq!(
            CliError::from(error).report(false, &mut stderr),
            ExitCode::from(2)
        );
        assert!(String::from_utf8(stderr).unwrap().ends_with(
            "  term \"tag:\"\n  at argument 1, byte 4\nhint: Run `ppn ls --help` for filter examples.\n"
        ));
    }

    #[test]
    fn a_saved_view_conflict_preserves_referrers_and_suggests_force() {
        let error = AppError::Conflict {
            field: "name".into(),
            reference: pepino_app::ConflictReference::View {
                view: "weekend".into(),
                referenced_by: vec!["maintenance".into()],
            },
        };
        let mut stderr = Vec::new();
        assert_eq!(
            CliError::from(error).report(true, &mut stderr),
            ExitCode::from(1)
        );
        let diagnostic: Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(
            diagnostic["error"]["referenced_by"],
            serde_json::json!(["maintenance"])
        );
        assert_eq!(
            diagnostic["error"]["hint"],
            "Pass `--force` to remove the referenced view."
        );
    }

    #[test]
    fn generic_application_errors_have_no_hint() {
        for error in [
            AppError::Storage {
                retryable: true,
                reason: "database busy".into(),
            },
            AppError::Busy,
            AppError::WriteCommitted {
                reason: "projection failed".into(),
            },
        ] {
            assert!(CliError::from(error).hint.is_none());
        }
    }

    #[test]
    fn renders_diagnostics_in_both_formats() {
        let error = CliError::new(ErrorKind::NotFound, "No task named Ship the release")
            .with_hint("Check the release board");
        assert_eq!(error.to_string(), "No task named Ship the release");
        let mut buffer = Vec::new();
        assert_eq!(error.report(false, &mut buffer), ExitCode::from(3));
        assert_eq!(
            String::from_utf8(buffer).unwrap(),
            "error: No task named Ship the release\nhint: Check the release board\n"
        );

        let mut buffer = Vec::new();
        assert_eq!(error.report(true, &mut buffer), ExitCode::from(3));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&buffer).unwrap(),
            serde_json::json!({"schema": 1, "kind": "error", "error": {"code": "not_found", "message": "No task named Ship the release", "hint": "Check the release board"}})
        );
    }
}
