mod cli;
mod debug;
mod help;
mod version;

fn main() -> std::process::ExitCode {
    cli::run()
}
