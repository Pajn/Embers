use clap::Parser;
use embers_cli::{Cli, Command, run};
use embers_core::init_tracing;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    // The detached server sets up its own rotating file logger in `run_server`;
    // every other invocation logs to stderr here.
    if !matches!(cli.command, Some(Command::Serve)) {
        init_tracing(&cli.log_filter());
    }

    if let Err(error) = run(cli).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
