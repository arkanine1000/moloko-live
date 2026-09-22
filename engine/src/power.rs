//! Whether the machine runs on battery. Kernel uevents say when to look again.

use std::fs;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::PathBuf;

use rustix::net::{self, AddressFamily, RecvFlags, SocketFlags, SocketType, netlink};

use crate::Result;

/// Room for one batch of uevents.
const UEVENT_BUFFER: usize = 8192;

pub struct Power {
    mains: Vec<PathBuf>,
    uevents: Option<OwnedFd>,
}

impl Power {
    pub fn open() -> Power {
        let mains = fs::read_dir("/sys/class/power_supply")
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|p| fs::read_to_string(p.join("type")).is_ok_and(|t| t.trim() == "Mains"))
            .collect();
        let uevents = uevent_socket()
            .map_err(|e| eprintln!("molokolive: no kernel uevents ({e}); plugging in or out goes unnoticed"))
            .ok();
        Power { mains, uevents }
    }

    /// A mains supply exists and none of them is online. Machines without one count as on mains.
    pub fn on_battery(&self) -> bool {
        !self.mains.is_empty()
            && !self
                .mains
                .iter()
                .any(|p| fs::read_to_string(p.join("online")).is_ok_and(|v| v.trim() == "1"))
    }

    /// Readable when uevents arrive; call `drain` then.
    pub fn fd(&self) -> Option<BorrowedFd<'_>> {
        self.uevents.as_ref().map(|fd| fd.as_fd())
    }

    /// Read every queued uevent; whether any concerned a power supply.
    pub fn drain(&self) -> bool {
        let Some(fd) = &self.uevents else { return false };
        let mut buf = [0u8; UEVENT_BUFFER];
        let mut power = false;
        while let Ok((n, _)) = net::recv(fd, &mut buf[..], RecvFlags::DONTWAIT) {
            if n == 0 {
                break;
            }
            let pattern = b"SUBSYSTEM=power_supply";
            power |= buf[..n.min(buf.len())].windows(pattern.len()).any(|w| w == pattern);
        }
        power
    }
}

fn uevent_socket() -> Result<OwnedFd> {
    let flags = SocketFlags::CLOEXEC | SocketFlags::NONBLOCK;
    let fd = net::socket_with(
        AddressFamily::NETLINK,
        SocketType::DGRAM,
        flags,
        Some(netlink::KOBJECT_UEVENT),
    )?;
    net::bind(&fd, &netlink::SocketAddrNetlink::new(0, 1))?;
    Ok(fd)
}
