#![deny(clippy::print_stdout)]

mod cli;
mod version;

fn main() -> std::process::ExitCode {
    cli::run()
}
