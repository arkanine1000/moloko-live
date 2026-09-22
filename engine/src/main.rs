//! molokolive: scenes from "Milk outside a bag of milk outside a bag of milk", animated behind the desktop.
//!
//! One binary: with a command it talks to the running wallpaper over its socket, otherwise it is the wallpaper.
//! The design is described in docs/architecture.md.

mod cli;
mod control;
mod engine;
mod frame;
mod geometry;
mod i3;
mod output;
mod pack;
mod pause;
mod power;
mod render;
mod rotation;
mod scenes;
mod show;
mod thermal;

use std::process::ExitCode;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// println! that stops quietly when stdout is gone (e.g. `molokolive --scenes --list | head`) instead of panicking.
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}
pub(crate) use say;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let fail = |message: String| {
        eprintln!("molokolive: {message}");
        ExitCode::FAILURE
    };
    if let Some(command) = argv.first().filter(|a| !a.starts_with('-')) {
        if control::Command::parse(command).is_none() {
            return fail(format!(
                "unknown command '{command}' (commands: {})",
                control::Command::list()
            ));
        }
        if argv.len() > 1 {
            return fail(format!("'{command}' doesn't take options"));
        }
        return match control::send(command) {
            Ok(reply) if reply.starts_with("error") => fail(reply.trim_start_matches("error: ").to_string()),
            Ok(reply) => {
                say!("{reply}");
                ExitCode::SUCCESS
            }
            Err(e) => fail(e.to_string()),
        };
    }
    match cli::parse(argv).and_then(engine::run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(e.to_string()),
    }
}
