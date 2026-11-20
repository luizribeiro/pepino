mod cli;
mod commands;
mod help;
mod task_stdin;

fn main() -> std::process::ExitCode {
    cli::run()
}
