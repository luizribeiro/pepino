mod cli;
mod commands;
mod task_json;

fn main() -> std::process::ExitCode {
    cli::run()
}
