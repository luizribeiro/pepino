mod add;
mod cli;
mod config_command;
mod debug;
mod help;
mod status;
mod version;

fn main() -> std::process::ExitCode {
    cli::run()
}
