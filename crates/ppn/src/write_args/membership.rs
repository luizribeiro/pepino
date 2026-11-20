use clap::Args;

#[derive(Args, Default)]
pub struct AddMembershipArgs {
    /// Add a tag. Repeat for more tags.
    #[arg(short = 't', long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,
    /// Assign me or a full identity ID. Repeat for more assignees.
    #[arg(long = "assign", value_name = "WHO")]
    pub assignees: Vec<String>,
}
