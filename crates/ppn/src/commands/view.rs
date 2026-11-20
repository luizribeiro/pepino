use clap::Subcommand;
use pepino_app::{ReadRequest, Request, SavedView, WriteRequest};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::response::ResponseExt;
use ppn::terminal::Context;

#[path = "../render/view.rs"]
mod render;

#[derive(clap::Args)]
#[command(about = "Manage saved task filters.", after_help = example_help(&["ppn view ls", "ppn view ls --json"]))]
pub struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Save a named filter for ppn ls @NAME, replacing a view with the same name.
    #[command(after_help = example_help(&[
        "ppn view save weekend tag:hardware 'due:<fri'",
        "ppn view save weekend tag:hardware --json",
    ]))]
    Save {
        /// Name of the view to save.
        name: String,
        /// Filter terms to save. See ppn help filters.
        #[arg(required_unless_present = "filter_text", value_name = "FILTER", value_parser = crate::cli::filter::unescape_term)]
        filter: Vec<String>,
        /// Parse a complete filter string, ANDed with any positional terms.
        #[arg(
            short = 'f',
            long = "filter",
            value_name = "STRING",
            allow_hyphen_values = true
        )]
        filter_text: Option<String>,
    },

    /// List saved view names and filters.
    #[command(after_help = example_help(&["ppn view ls", "ppn view ls --json"]))]
    Ls,

    /// Remove a saved view.
    #[command(after_help = example_help(&["ppn view rm weekend --force", "ppn view rm weekend --json"]))]
    Rm {
        /// Name of the view to remove.
        name: String,
        /// Remove the view even if other saved views reference it.
        #[arg(long)]
        force: bool,
    },

    /// Show the canonical filter of a saved view.
    #[command(after_help = example_help(&["ppn view show weekend", "ppn view show weekend --json"]))]
    Show {
        /// Name of the view to display.
        name: String,
    },
}

impl Args {
    pub(crate) fn writes(&self) -> bool {
        matches!(self.action, Action::Save { .. } | Action::Rm { .. })
    }
}

pub fn run(context: &Context<'_>, args: Args) -> Result<CommandResult, CliError> {
    let mut client = AppClient::open(context)?;
    let response = match args.action {
        Action::Save {
            name,
            filter,
            filter_text,
        } => {
            let (filter, positions) = crate::cli::filter::input(filter_text, filter)?;
            client
                .call(Request::Write(WriteRequest::SaveView {
                    name: name.clone(),
                    filter,
                }))
                .map_err(|error| positions.remap(error, Some(&name)))
        }
        Action::Ls => client.call(Request::Read(ReadRequest::ListViews)),
        Action::Rm { name, force } => {
            client.call(Request::Write(WriteRequest::DeleteView { name, force }))
        }
        Action::Show { name } => {
            let response = client.call(Request::Read(ReadRequest::ListViews))?;
            let view = SavedView::find_by_name(response.views()?.views, &name)?.clone();
            return Ok(CommandResult::document(render::ViewDefinition(view)));
        }
    }?;
    let empty = matches!(&response, pepino_app::Response::Views { views } if views.is_empty());
    Ok(
        CommandResult::app_document(context, response, |response, _| {
            render::saved_views(response)
        })?
        .with_human_note(empty.then(|| "No saved views.".into())),
    )
}
