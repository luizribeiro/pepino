use pepino_app::Request;
use ppn::app::AppClient;
use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::input::read_text;
use ppn::output::CommandResult;
use ppn::render::write_document;
use ppn::terminal::Context;
use ppn::write_args::{TaskWriteArgs, new_task};

#[derive(clap::Args)]
/// Create a task.
#[command(after_help = example_help(&[
    "ppn add 'Replace the greenhouse latch' -t hardware --due tomorrow",
    "ppn add 'Order screws' --json",
]))]
pub struct Args {
    /// Task title. Use - for stdin text or, with --json, a JSON add_task object.
    title: String,
    #[command(flatten)]
    args: Box<TaskWriteArgs>,
}

pub fn run(context: &Context<'_>, input: Args, json: bool) -> Result<CommandResult, CliError> {
    let Args { title, args } = input;
    let from_stdin = title == "-";
    if from_stdin && args.fields.description.reads_stdin() {
        return Err(CliError::new(
            ErrorKind::Usage,
            "The title and description both use stdin.",
        )
        .with_hint("Pass the description as text or use --from-file."));
    }
    let input = read_text(Some(title), None)?;
    let request = if from_stdin {
        crate::task_stdin::request(&input, json)?
    } else {
        new_task(input)
    };
    let request = args.apply_to_add(request, context.time.today)?;
    let response = AppClient::open(context)?.call(Request::Write(request))?;
    write_document(context, response, "Created")
}
