use std::path::PathBuf;

use pepino_app::{Request, WriteRequest};
use ppn::app::AppClient;
use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::input::read_task_text;
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
    /// Task title. Use - to read title text from stdin.
    #[arg(required_unless_present = "from_json")]
    title: Vec<String>,
    /// Read a task object from a file or - for stdin. See ppn help json.
    #[arg(long, value_name = "FILE|-", conflicts_with = "title")]
    from_json: Option<PathBuf>,
    #[command(flatten)]
    args: Box<TaskWriteArgs>,
}

pub fn run(context: &Context<'_>, input: Args) -> Result<CommandResult, CliError> {
    let mut client = AppClient::open(context)?;
    let Args {
        title,
        from_json,
        args,
    } = input;
    let title = (!title.is_empty()).then(|| title.join(" "));
    let from_stdin = title.as_deref() == Some("-")
        || from_json
            .as_deref()
            .is_some_and(|path| path == std::path::Path::new("-"));
    if from_stdin && args.fields.description.reads_stdin() {
        return Err(CliError::new(
            ErrorKind::Usage,
            "The task input and description both use stdin.",
        )
        .with_hint("Pass the description as text or use --from-file."));
    }
    let json_input = from_json.is_some();
    let scalar_inputs = [
        ("status", args.fields.status.is_some()),
        ("due", args.fields.due.is_some()),
        ("priority", args.fields.priority.is_some()),
        ("parent", args.relations.parent.is_some()),
    ];
    let request = match from_json {
        Some(file) => crate::task_json::request(&file, context.time.today)?,
        None => new_task(read_task_text(title, None)?),
    };
    let description = args.fields.description.input();
    let set_keys: Vec<_> = args.fields.custom.set_keys().map(str::to_owned).collect();
    let supplied = match &request {
        WriteRequest::AddTask {
            tags,
            assignees,
            blocks,
            blocked_by,
            ..
        } => [tags.len(), assignees.len(), blocks.len(), blocked_by.len()],
        _ => [0; 4],
    };
    let request = args.apply_to_add(request, context.time.today)?;
    let response = client.call(Request::Write(request)).map_err(|mut error| {
        for ((field, flag), count) in [
            ("tags", "--tag"),
            ("assignees", "--assign"),
            ("blocks", "--blocks"),
            ("blocked_by", "--blocked-by"),
        ]
        .into_iter()
        .zip(supplied)
        {
            let source = if error.input_index().is_some_and(|index| index < count) {
                "--from-json"
            } else {
                flag
            };
            error = error.map_input(field, source);
        }
        if let Some(source) = description.or(json_input.then_some("--from-json")) {
            error = error.map_input("description", source);
        }
        if json_input {
            error = error.map_input("title", "--from-json");
            if !error
                .input_key("fields")
                .is_some_and(|key| set_keys.iter().any(|flag_key| *flag_key == key))
            {
                error = error.map_input("fields", "--from-json");
            }
            for (field, supplied_by_flag) in scalar_inputs {
                if !supplied_by_flag {
                    error = error.map_input(field, "--from-json");
                }
            }
        }
        error
    })?;
    write_document(context, response, "Created")
}
