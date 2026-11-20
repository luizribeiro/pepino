mod cli;
mod config_command;
mod debug;
mod help;
mod version;

fn main() -> std::process::ExitCode {
    cli::run()
}
