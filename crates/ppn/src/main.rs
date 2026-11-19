use clap::Parser;

#[derive(Parser)]
#[command(
    version,
    about = "An experimental local-first, peer-to-peer task manager"
)]
struct Cli {}

fn main() {
    Cli::parse();
}
