mod cli;
mod commands;
mod task_stdin;

fn main() -> std::process::ExitCode {
    cli::run()
}
