//! The control socket, `$XDG_RUNTIME_DIR/molokolive.sock`: `molokolive next` and friends connect, send one command
//! line and print the engine's one-line reply. The engine polls the listening socket along with its other events.

use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

use crate::Result;

/// How long a client gets to send its line, and the engine to answer.
const CLIENT_TIMEOUT: Duration = Duration::from_millis(200);
/// How long a client waits for the engine's reply.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// What a client can ask the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Next,
    Prev,
    SkyNext,
    SkyPrev,
    Pause,
    Resume,
    Status,
}

impl Command {
    const NAMES: [(&str, Command); 7] = [
        ("next", Command::Next),
        ("prev", Command::Prev),
        ("sky-next", Command::SkyNext),
        ("sky-prev", Command::SkyPrev),
        ("pause", Command::Pause),
        ("resume", Command::Resume),
        ("status", Command::Status),
    ];

    pub fn parse(text: &str) -> Option<Command> {
        Command::NAMES
            .iter()
            .find(|(name, _)| *name == text)
            .map(|&(_, command)| command)
    }

    /// The command names, comma separated, for messages.
    pub fn list() -> String {
        Command::NAMES
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
        .unwrap_or_else(std::env::temp_dir)
        .join("molokolive.sock")
}

pub struct Server {
    listener: UnixListener,
}

/// One client's command, answered with `reply`.
pub struct Request {
    stream: UnixStream,
    pub command: String,
}

impl Server {
    /// Take over the socket path. Call after publishing, which has already ended an engine that was running.
    pub fn bind() -> Result<Server> {
        let path = socket_path();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        listener.set_nonblocking(true)?;
        Ok(Server { listener })
    }

    /// Readable when a client connects; call `accept` then.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.listener.as_fd()
    }

    /// Every waiting client's command. A client that doesn't send a line in time is dropped.
    pub fn accept(&self) -> Vec<Request> {
        let mut requests = Vec::new();
        while let Ok((stream, _)) = self.listener.accept() {
            let read = || -> Result<Request> {
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(CLIENT_TIMEOUT))?;
                stream.set_write_timeout(Some(CLIENT_TIMEOUT))?;
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line)?;
                Ok(Request {
                    command: line.trim().to_string(),
                    stream: stream.try_clone()?,
                })
            };
            if let Ok(request) = read() {
                requests.push(request);
            }
        }
        requests
    }
}

impl Request {
    pub fn reply(mut self, text: &str) {
        let _ = self.stream.write_all(format!("{text}\n").as_bytes());
    }
}

/// Send `command` to the running engine and return its reply.
pub fn send(command: &str) -> Result<String> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path)
        .map_err(|_| format!("molokolive isn't running (nothing is listening on {})", path.display()))?;
    stream.set_read_timeout(Some(REPLY_TIMEOUT))?;
    stream.write_all(format!("{command}\n").as_bytes())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    Ok(reply.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::Command;

    #[test]
    fn commands_parse_by_name() {
        assert_eq!(Command::parse("sky-next"), Some(Command::SkyNext));
        assert_eq!(Command::parse("dance"), None);
        assert_eq!(Command::list(), "next, prev, sky-next, sky-prev, pause, resume, status");
    }
}
