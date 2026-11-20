mod cli;
mod commands;
mod config_command;
mod debug;
mod help;
mod show;
mod status;
mod task_stdin;
mod version;

fn main() -> std::process::ExitCode {
    cli::run()
}
