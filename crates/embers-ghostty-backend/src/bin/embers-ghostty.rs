//! Embers with panes emulated by libghostty-vt instead of alacritty: the
//! same CLI and client, with a server of its own.
//!
//! Its default socket is `embers-ghostty.sock`, beside Embers' own, so it
//! keeps its own sessions and saved layout and never talks to an Embers
//! server that emulates with alacritty. `--socket` still picks another.

use std::process::ExitCode;

use clap::Parser;
use embers_cli::{Cli, Command, default_runtime_dir, run};
use embers_core::{PtySize, init_tracing};
use embers_ghostty_backend::GhosttyTerminalBackend;
use embers_server::{
    RUNTIME_KEEPER_BIN_ENV_VAR, SOCKET_ENV_VAR, TerminalBackend, set_terminal_backend,
};

fn ghostty(size: PtySize, max_scrollback_lines: usize) -> Box<dyn TerminalBackend> {
    Box::new(GhosttyTerminalBackend::new(size, max_scrollback_lines))
}

fn main() -> ExitCode {
    set_terminal_backend(ghostty);
    // SAFETY: no other thread exists yet to read the environment.
    unsafe {
        // The server it starts, and keepers that server starts, are this
        // binary too, so they emulate with Ghostty as well.
        if let Ok(executable) = std::env::current_exe() {
            std::env::set_var(RUNTIME_KEEPER_BIN_ENV_VAR, executable);
        }
        // Inside a pane of another Embers server, `EMBERS_SOCKET` names that
        // server's socket.
        std::env::set_var(
            SOCKET_ENV_VAR,
            default_runtime_dir().join("embers-ghostty.sock"),
        );
    }

    let cli = Cli::parse();
    // The detached server sets up its own rotating file logger in
    // `run_server`; every other invocation logs to stderr here.
    if !matches!(cli.command, Some(Command::Serve)) {
        init_tracing(&cli.log_filter());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the Tokio runtime starts");
    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
