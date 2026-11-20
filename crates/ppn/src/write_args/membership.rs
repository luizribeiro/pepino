use clap::Args;
use pepino_app::WriteRequest;

#[derive(Args, Default)]
pub struct AddMembershipArgs {
    /// Add a tag. Repeat for more tags.
    #[arg(short = 't', long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,
    /// Assign me or a full identity ID. Repeat for more assignees.
    #[arg(long = "assign", value_name = "WHO")]
    pub assignees: Vec<String>,
}

#[derive(Args, Default)]
pub struct MembershipArgs {
    #[command(flatten)]
    pub add: AddMembershipArgs,
    /// Remove a tag. Repeat for more tags.
    #[arg(long = "untag", value_name = "TAG")]
    pub remove_tags: Vec<String>,
    /// Unassign me or a full identity ID. Repeat for more assignees.
    #[arg(long = "unassign", value_name = "WHO")]
    pub remove_assignees: Vec<String>,
}

impl MembershipArgs {
    pub fn requests(self, task: &str) -> Vec<WriteRequest> {
        let mut requests = Vec::new();
        let Self {
            add,
            remove_tags,
            remove_assignees,
        } = self;
        if !add.tags.is_empty() {
            requests.push(WriteRequest::AddTags {
                task: task.into(),
                tags: add.tags,
            });
        }
        if !remove_tags.is_empty() {
            requests.push(WriteRequest::RemoveTags {
                task: task.into(),
                tags: remove_tags,
            });
        }
        if !add.assignees.is_empty() {
            requests.push(WriteRequest::Assign {
                task: task.into(),
                assignees: add.assignees,
            });
        }
        if !remove_assignees.is_empty() {
            requests.push(WriteRequest::Unassign {
                task: task.into(),
                assignees: remove_assignees,
            });
        }
        requests
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use serde_json::json;

    #[derive(Parser)]
    struct Edit {
        #[command(flatten)]
        membership: MembershipArgs,
    }

    #[test]
    fn only_nonempty_membership_operations_become_requests() {
        let edit = Edit::parse_from([
            "edit",
            "-t",
            "hardware",
            "--untag",
            "wood",
            "--unassign",
            "me",
        ]);
        assert_eq!(
            serde_json::to_value(edit.membership.requests("97ad97")).unwrap(),
            json!([
                {"op":"add_tags","task":"97ad97","tags":["hardware"]},
                {"op":"remove_tags","task":"97ad97","tags":["wood"]},
                {"op":"unassign","task":"97ad97","assignees":["me"]}
            ])
        );
        assert!(MembershipArgs::default().requests("97ad97").is_empty());
    }
}
