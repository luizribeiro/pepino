use pepino_app::Request;
use ppn::app::AppClient;
use ppn::error::{CliError, ErrorKind};
use ppn::input::read_text;
use ppn::output::CommandResult;
use ppn::render::write_document;
use ppn::terminal::Context;
use ppn::write_args::{TaskWriteArgs, new_task};

pub fn run(
    context: &Context<'_>,
    title: String,
    args: TaskWriteArgs,
    json: bool,
) -> Result<CommandResult, CliError> {
    let from_stdin = title == "-";
    if from_stdin && args.description.reads_stdin() {
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
