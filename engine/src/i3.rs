//! i3 IPC: whether the desktop behind the windows can be seen, and events for when that may have changed.
//!
//! The desktop counts as covered when a visible workspace holds any tiled window (with `smart_gaps on`, one tiled
//! window hides the wallpaper completely) or a fullscreen window. Floating windows leave most of it visible and don't
//! count. One connection receives workspace/window/output events; a second one answers the tree queries.

use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;

use serde_json::Value;

use crate::Result;

const MAGIC: &[u8; 6] = b"i3-ipc";
const GET_WORKSPACES: u32 = 1;
const SUBSCRIBE: u32 = 2;
const GET_TREE: u32 = 4;
const EVENT: u32 = 1 << 31;
const SHUTDOWN_EVENT: u32 = EVENT | 6;

pub struct I3 {
    events: UnixStream,
    commands: UnixStream,
    pending: Vec<u8>,
}

/// What a batch of events means for the engine.
pub enum Events {
    /// Nothing that could change visibility (or nothing at all).
    None,
    /// Recheck visibility.
    Changed,
    /// i3 is restarting or exiting; the connection is gone.
    Lost,
}

impl I3 {
    pub fn connect(path: &str) -> Result<I3> {
        let connect = || UnixStream::connect(path).map_err(|e| format!("i3 IPC {path}: {e}"));
        let (mut events, commands) = (connect()?, connect()?);
        send(&mut events, SUBSCRIBE, br#"["workspace","window","output","shutdown"]"#)?;
        let (_, reply) = read_message(&mut events)?;
        if serde_json::from_slice::<Value>(&reply)?.get("success") != Some(&Value::Bool(true)) {
            return Err("i3 IPC: subscribing to events failed".into());
        }
        events.set_nonblocking(true)?;
        Ok(I3 { events, commands, pending: Vec::new() })
    }

    /// Readable when events arrive; call `drain` then.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.events.as_fd()
    }

    /// Read every event that has arrived.
    pub fn drain(&mut self) -> Result<Events> {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match self.events.read(&mut buf) {
                Ok(0) => return Ok(Events::Lost),
                Ok(n) => self.pending.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) if matches!(e.kind(), ErrorKind::ConnectionReset | ErrorKind::BrokenPipe) => return Ok(Events::Lost),
                Err(e) => return Err(e.into()),
            }
        }
        let mut result = Events::None;
        while self.pending.len() >= 14 {
            if &self.pending[..6] != MAGIC {
                return Err("i3 IPC: bad message header".into());
            }
            let length = u32::from_le_bytes(self.pending[6..10].try_into()?) as usize;
            if self.pending.len() < 14 + length {
                break;
            }
            let kind = u32::from_le_bytes(self.pending[10..14].try_into()?);
            self.pending.drain(..14 + length);
            if kind == SHUTDOWN_EVENT {
                return Ok(Events::Lost);
            }
            if kind & EVENT != 0 {
                result = Events::Changed;
            }
        }
        Ok(result)
    }

    /// Whether no visible workspace is covered by a tiled or fullscreen window.
    pub fn desktop_visible(&mut self) -> Result<bool> {
        let workspaces = self.request(GET_WORKSPACES)?;
        let visible: Vec<&str> = workspaces
            .as_array()
            .into_iter()
            .flatten()
            .filter(|w| w["visible"] == Value::Bool(true))
            .filter_map(|w| w["name"].as_str())
            .collect();
        let tree = self.request(GET_TREE)?;
        let mut shown = Vec::new();
        find_workspaces(&tree, &mut shown);
        let covered = shown
            .iter()
            .filter(|w| w["name"].as_str().is_some_and(|n| visible.contains(&n)))
            .any(|w| has_tiled_window(w) || has_fullscreen(w))
            || has_global_fullscreen(&tree);
        Ok(!covered)
    }

    fn request(&mut self, kind: u32) -> Result<Value> {
        send(&mut self.commands, kind, b"")?;
        loop {
            let (reply_kind, payload) = read_message(&mut self.commands)?;
            if reply_kind == kind {
                return Ok(serde_json::from_slice(&payload)?);
            }
        }
    }
}

fn send(stream: &mut UnixStream, kind: u32, payload: &[u8]) -> Result<()> {
    let mut message = Vec::with_capacity(14 + payload.len());
    message.extend_from_slice(MAGIC);
    message.extend_from_slice(&u32::try_from(payload.len())?.to_le_bytes());
    message.extend_from_slice(&kind.to_le_bytes());
    message.extend_from_slice(payload);
    stream.write_all(&message)?;
    Ok(())
}

fn read_message(stream: &mut UnixStream) -> Result<(u32, Vec<u8>)> {
    let mut header = [0u8; 14];
    stream.read_exact(&mut header)?;
    if &header[..6] != MAGIC {
        return Err("i3 IPC: bad message header".into());
    }
    let length = u32::from_le_bytes(header[6..10].try_into()?) as usize;
    let kind = u32::from_le_bytes(header[10..14].try_into()?);
    let mut payload = vec![0u8; length];
    stream.read_exact(&mut payload)?;
    Ok((kind, payload))
}

fn children(node: &Value) -> impl Iterator<Item = &Value> {
    ["nodes", "floating_nodes"].into_iter().flat_map(move |k| node[k].as_array().into_iter().flatten())
}

fn find_workspaces<'a>(node: &'a Value, out: &mut Vec<&'a Value>) {
    if node["type"] == "workspace" {
        out.push(node);
        return;
    }
    children(node).for_each(|c| find_workspaces(c, out));
}

/// A window anywhere under the tiling tree (`nodes`), not in a floating container.
fn has_tiled_window(node: &Value) -> bool {
    node["nodes"].as_array().into_iter().flatten().any(|c| c["window"].is_u64() || has_tiled_window(c))
}

/// A fullscreen container on this workspace. Workspaces themselves always report fullscreen_mode 1, so only
/// containers count.
fn has_fullscreen(node: &Value) -> bool {
    children(node).any(|c| (c["type"] == "con" && c["fullscreen_mode"].as_u64().is_some_and(|m| m > 0)) || has_fullscreen(c))
}

/// A container in global fullscreen (mode 2) covers every output.
fn has_global_fullscreen(node: &Value) -> bool {
    children(node).any(|c| c["fullscreen_mode"].as_u64() == Some(2) || has_global_fullscreen(c))
}
