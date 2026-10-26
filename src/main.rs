//! linearctl: an agent-first CLI for Linear.
//!
//! It speaks GraphQL to api.linear.app directly (no SDK); every document it
//! sends is in queries.rs and checked against graphql/schema.graphql by the
//! tests. What it prints, and the exit codes, are its interface: see
//! docs/design.md.

mod cli;
mod client;
mod commands;
mod demo;
mod fail;
#[cfg(test)]
mod generic;
mod gql;
mod json;
mod keys;
mod queries;
mod run;
mod state;
#[cfg(test)]
mod testserver;
mod time;
mod util;

use run::{Sink, execute};

fn main() {
    interrupts();
    let args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    let (code, _, _) = execute(&args, Sink::Stdout, Sink::Stderr);
    std::process::exit(code);
}

/// Ctrl-C (or SIGTERM) removes a temp file this run made, then exits 130
/// (143): a crash or an interrupt never leaves one behind.
fn interrupts() {
    #[cfg(unix)]
    {
        // A reader that stops early (| head) ends the run quietly, as Go's
        // runtime did, rather than with "Broken pipe" on stderr: Rust
        // ignores SIGPIPE by default.
        // SAFETY: restoring the default action, before any thread starts.
        unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
        use signal_hook::consts::{SIGINT, SIGTERM};
        let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGINT, SIGTERM]) else { return };
        std::thread::spawn(move || {
            if let Some(s) = signals.forever().next() {
                state::remove_ours();
                std::process::exit(if s == SIGINT { 130 } else { 143 });
            }
        });
    }
    // Ctrl-C and Ctrl-Break are Go's os.Interrupt there; closing the
    // console, logging off or shutting down are its SIGTERM.
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};
        unsafe extern "system" fn handler(event: u32) -> windows_sys::core::BOOL {
            state::remove_ours();
            std::process::exit(if event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT { 130 } else { 143 });
        }
        // SAFETY: handler is a valid function for the life of the process.
        unsafe { SetConsoleCtrlHandler(Some(handler), 1) };
    }
}
