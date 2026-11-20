use pepino_app::{EdgeKind, Request, WriteRequest};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::relation_document;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(about = "Link two tasks.", after_help = example_help(&[
    "ppn link 97ad97 blocks 33ea33", "ppn link 97ad97 parent 33ea33 --json",
]))]
pub struct Args {
    #[command(flatten)]
    relation: RelationArgs,
}

#[derive(clap::Args)]
pub(super) struct RelationArgs {
    /// First task ID or unambiguous prefix.
    #[arg(value_name = "FROM")]
    from: String,
    /// Read the link as FROM blocks TO, FROM has parent TO, or FROM relates to TO.
    /// Use blocked-by to reverse a blocking link.
    #[arg(value_enum)]
    kind: Relation,
    /// Second task ID or unambiguous prefix.
    #[arg(value_name = "TO")]
    to: String,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Relation {
    Blocks,
    BlockedBy,
    Parent,
    Relates,
}

impl RelationArgs {
    pub(super) fn endpoints(self) -> (String, EdgeKind, String) {
        let Self { from, kind, to } = self;
        match kind {
            Relation::Blocks => (from, EdgeKind::Blocks, to),
            Relation::BlockedBy => (to, EdgeKind::Blocks, from),
            Relation::Parent => (from, EdgeKind::Parent, to),
            Relation::Relates => (from, EdgeKind::Relates, to),
        }
    }
}

pub fn run(context: &Context<'_>, Args { relation }: Args) -> Result<CommandResult, CliError> {
    let (from, kind, to) = relation.endpoints();
    let response = AppClient::open(context)?.call(Request::Write(WriteRequest::Link {
        from: from.clone(),
        kind,
        to: to.clone(),
    }))?;
    relation_document(context, response, "Linked", (&from, kind, &to))
}
