mod cli;
mod commands;
mod help;
mod task_stdin;
mod version;

fn main() -> std::process::ExitCode {
    cli::run()
}
