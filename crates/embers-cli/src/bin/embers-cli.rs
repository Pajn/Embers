use clap::Parser;
use embers_cli::{Cli, run};
use embers_core::init_tracing;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    init_tracing(&cli.log_filter());

    if let Err(error) = run(cli).await {
        eprintln!("{}", format_error_chain(&error));
        std::process::exit(1);
    }
}

fn format_error_chain(error: &dyn std::error::Error) -> String {
    let mut rendered = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        rendered.push_str("\ncaused by: ");
        rendered.push_str(&cause.to_string());
        source = cause.source();
    }
    rendered
}
