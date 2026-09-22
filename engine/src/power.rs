//! Power and heat: whether the machine runs on battery (kernel uevents say when to check), and the CPU package
//! temperature (no events exist for it, so the engine reads it on a slow timer, and only while nothing else has
//! stopped it).

use std::fs;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::PathBuf;

use rustix::net::{self, AddressFamily, RecvFlags, SocketFlags, SocketType, netlink};

use crate::Result;

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
        let mut buf = [0u8; 8192];
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

pub struct Thermal {
    path: PathBuf,
    /// Stop at or above this temperature (°C).
    pub max: i32,
}

impl Thermal {
    /// Resume only once the temperature is this many degrees below the maximum.
    pub const HYSTERESIS: i32 = 5;

    /// The x86_pkg_temp thermal zone, which follows the CPU package closely.
    pub fn open(max: i32) -> Result<Thermal> {
        let mut types = Vec::new();
        for entry in fs::read_dir("/sys/class/thermal")?.flatten() {
            let Ok(kind) = fs::read_to_string(entry.path().join("type")) else {
                continue;
            };
            if kind.trim() == "x86_pkg_temp" {
                return Ok(Thermal {
                    path: entry.path().join("temp"),
                    max,
                });
            }
            types.push(kind.trim().to_string());
        }
        Err(format!("--max-temp: no x86_pkg_temp thermal zone (found: {})", types.join(", ")).into())
    }

    pub fn celsius(&self) -> Result<i32> {
        let millidegrees: i32 = fs::read_to_string(&self.path)?.trim().parse()?;
        Ok(millidegrees / 1000)
    }

    /// Whether to be stopped at `celsius`, given whether we already are.
    pub fn too_hot(&self, celsius: i32, stopped: bool) -> bool {
        if stopped {
            celsius > self.max - Self::HYSTERESIS
        } else {
            celsius >= self.max
        }
    }
}
