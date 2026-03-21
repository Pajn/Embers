use clap::Parser;
use embers_cli::{Cli, run};
use embers_core::init_tracing;

#[tokio::main]
async fn main() {
    init_tracing("info");

    let cli = Cli::parse();
    if let Err(error) = run(cli).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
