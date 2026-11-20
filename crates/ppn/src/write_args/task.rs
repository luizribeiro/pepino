use std::path::PathBuf;

use clap::Args;

use super::FieldArgs;
use crate::error::CliError;
use crate::input::read_text;

#[derive(Args, Default)]
pub struct TaskWriteArgs {
    /// Add a tag. Repeat for more tags.
    #[arg(short = 't', long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,
    /// Set the due date. Accepts YYYY-MM-DD or relative dates such as tomorrow and -2w.
    #[arg(long, allow_hyphen_values = true, value_name = "DATE")]
    pub due: Option<String>,
    /// Set the task priority as a signed integer.
    #[arg(long, allow_negative_numbers = true)]
    pub priority: Option<i64>,
    /// Assign me or a full identity ID. Repeat for more assignees.
    #[arg(long = "assign", value_name = "WHO")]
    pub assignees: Vec<String>,
    #[command(flatten)]
    pub relations: RelationArgs,
    #[command(flatten)]
    pub fields: FieldArgs,
    #[command(flatten)]
    pub description: DescriptionArgs,
}

#[derive(Args, Default)]
pub struct RelationArgs {
    /// Set the parent task ID or prefix.
    #[arg(long, value_name = "TASK")]
    pub parent: Option<String>,
    /// Block a task by ID or prefix. Repeat for more dependencies.
    #[arg(long, value_name = "TASK")]
    pub blocks: Vec<String>,
    /// Depend on a task by ID or prefix. Repeat for more dependencies.
    #[arg(long, value_name = "TASK")]
    pub blocked_by: Vec<String>,
}

impl RelationArgs {
    pub(super) fn apply(self, request: &mut pepino_app::WriteRequest) {
        if let pepino_app::WriteRequest::AddTask {
            parent,
            blocks,
            blocked_by,
            ..
        } = request
        {
            *parent = self.parent.or_else(|| parent.take());
            blocks.extend(self.blocks);
            blocked_by.extend(self.blocked_by);
        }
    }
}

#[derive(Args, Default)]
pub struct DescriptionArgs {
    /// Set the description. Use - to read it from stdin.
    #[arg(long, value_name = "TEXT")]
    description: Option<String>,
    /// Read the description from a file.
    #[arg(long, value_name = "PATH", conflicts_with = "description")]
    from_file: Option<PathBuf>,
}

impl DescriptionArgs {
    pub fn reads_stdin(&self) -> bool {
        self.description.as_deref() == Some("-")
    }

    pub fn read(self) -> Result<Option<String>, CliError> {
        if self.description.is_none() && self.from_file.is_none() {
            return Ok(None);
        }
        read_text(self.description, self.from_file.as_deref()).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Edit {
        #[command(flatten)]
        args: TaskWriteArgs,
    }

    #[test]
    fn parses_shared_flags_and_distinguishes_text_sources() {
        let edit = Edit::try_parse_from([
            "edit",
            "-t",
            "release",
            "--tag",
            "docs",
            "--due",
            "-1w",
            "--priority",
            "-2",
            "--assign",
            "me",
            "--description",
            "-",
        ])
        .unwrap()
        .args;
        assert_eq!(edit.tags, ["release", "docs"]);
        assert_eq!(edit.due.as_deref(), Some("-1w"));
        assert_eq!(edit.priority, Some(-2));
        assert_eq!(edit.assignees, ["me"]);
        assert!(edit.description.reads_stdin());
        let edit = Edit::try_parse_from(["edit", "--description", "Check release notes"]).unwrap();
        assert_eq!(
            edit.args.description.read().unwrap().as_deref(),
            Some("Check release notes")
        );
        assert_eq!(DescriptionArgs::default().read().unwrap(), None);
        assert!(
            Edit::try_parse_from(["edit", "--description", "-", "--from-file", "notes.txt"])
                .is_err()
        );
    }
}
