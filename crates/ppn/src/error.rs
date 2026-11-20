use std::io::{self, Write};
use std::process::ExitCode;

use pepino_app::{AppError, AppErrorKind};
use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum ErrorKind {
    Failure,
    Usage,
    NotFound,
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
            | Self::Busy => 1,
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
        let (message, details) = rendered
            .trim()
            .split_once('\n')
            .unwrap_or((rendered.trim(), ""));
        let mut error = Self::new(ErrorKind::Usage, message.trim_start_matches("error: "));
        if !details.trim().is_empty() {
            error = error.with_hint(details.trim());
        }
        error
    }

    pub fn report(&self, json: bool, mut stderr: impl Write) -> ExitCode {
        let _ = self.write(json, &mut stderr);
        self.kind.status().into()
    }

    fn write(&self, json: bool, stderr: &mut dyn Write) -> io::Result<()> {
        if json {
            serde_json::to_writer(&mut *stderr, &serde_json::json!({ "error": self }))?;
            return writeln!(stderr);
        }

        writeln!(stderr, "error: {}", self.message)?;
        if let Some(candidates) = self.details.get("candidates").and_then(Value::as_array) {
            for id in candidates.iter().filter_map(Value::as_str) {
                writeln!(stderr, "  {id}")?;
            }
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
            writeln!(stderr, "hint: {hint}")?;
        }
        Ok(())
    }
}

impl From<AppError> for CliError {
    fn from(error: AppError) -> Self {
        let (kind, hint) = match error.kind() {
            AppErrorKind::NotFound => (ErrorKind::NotFound, None),
            AppErrorKind::AmbiguousId => (ErrorKind::AmbiguousId, Some("Use a longer task ID.")),
            AppErrorKind::InvalidInput => (
                ErrorKind::InvalidInput,
                Some("Check the value at the reported field."),
            ),
            AppErrorKind::Conflict => (ErrorKind::Conflict, None),
            AppErrorKind::Busy => (ErrorKind::Busy, Some("Retry or increase --timeout.")),
            AppErrorKind::Storage => (
                ErrorKind::Failure,
                error.retryable().then_some("Retry the request."),
            ),
            AppErrorKind::UnsupportedVersion => (
                ErrorKind::Failure,
                Some("Use a ppn version that supports the app protocol."),
            ),
            AppErrorKind::WriteCommitted => (
                ErrorKind::Failure,
                Some("Read the current state before retrying."),
            ),
            _ => (ErrorKind::Failure, None),
        };
        let mut diagnostic = Self::new(kind, error.to_string());
        diagnostic.hint = hint.map(str::to_owned);
        if let Ok(Value::Object(mut details)) = serde_json::to_value(error) {
            details.remove("kind");
            if let Some(Value::String(hint)) = details.remove("hint") {
                diagnostic.hint = Some(hint);
            }
            diagnostic.details = details;
        }
        diagnostic
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
                .contains("at byte 2\nhint: Use tag")
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
    fn transport_errors_keep_retry_guidance() {
        for (error, hint) in [
            (
                AppError::Storage {
                    retryable: false,
                    reason: "bad schema".into(),
                },
                None,
            ),
            (
                AppError::Storage {
                    retryable: true,
                    reason: "database busy".into(),
                },
                Some("Retry the request."),
            ),
            (
                AppError::UnsupportedVersion {
                    requested: 2,
                    supported: 1,
                },
                Some("Use a ppn version that supports the app protocol."),
            ),
            (
                AppError::WriteCommitted {
                    reason: "projection failed".into(),
                },
                Some("Read the current state before retrying."),
            ),
        ] {
            let error = CliError::from(error);
            assert_eq!(error.kind(), ErrorKind::Failure);
            assert_eq!(error.hint.as_deref(), hint);
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
            serde_json::json!({"error": {"code": "not_found", "message": "No task named Ship the release", "hint": "Check the release board"}})
        );
    }
}
