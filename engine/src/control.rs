//! The control socket, `$XDG_RUNTIME_DIR/molokolive.sock`: `molokolive next` and friends connect, send one command
//! line and print the engine's one-line reply. The engine polls the listening socket along with its other events.

use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

use crate::Result;

pub const COMMANDS: &[&str] = &["next", "prev", "sky-next", "sky-prev", "pause", "resume", "status"];

pub fn socket_path() -> PathBuf {
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

    /// Every waiting client's command. A client that doesn't send a line within 200 ms is dropped.
    pub fn accept(&self) -> Vec<Request> {
        let mut requests = Vec::new();
        while let Ok((stream, _)) = self.listener.accept() {
            let read = || -> Result<Request> {
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(Duration::from_millis(200)))?;
                stream.set_write_timeout(Some(Duration::from_millis(200)))?;
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line)?;
                Ok(Request { command: line.trim().to_string(), stream: stream.try_clone()? })
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
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.write_all(format!("{command}\n").as_bytes())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    Ok(reply.trim_end().to_string())
}
