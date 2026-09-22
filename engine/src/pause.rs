//! Why the animation may stop: a covered desktop (i3), battery power, or heat. All three must be clear to run.

use std::os::fd::BorrowedFd;
use std::time::{Duration, Instant};

use crate::cli::Args;
use crate::i3::{Events, I3};
use crate::output::Output;
use crate::power::Power;
use crate::thermal::Thermal;
use crate::{Result, say};

/// How often the temperature is read while animating, or while stopped for heat.
const TEMPERATURE_INTERVAL: Duration = Duration::from_secs(5);
/// How long to wait before reconnecting after i3 restarts.
const I3_RETRY: Duration = Duration::from_secs(2);

/// Why animation is stopped, if it is.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stops {
    pub covered: bool,
    pub battery: bool,
    pub hot: Option<i32>,
    pub manual: bool,
}

impl Stops {
    pub fn running(&self) -> bool {
        !self.covered && !self.battery && self.hot.is_none() && !self.manual
    }

    /// "animating", or "paused: " and the reasons. Part of the control socket's replies.
    pub fn describe(&self) -> String {
        let mut reasons = Vec::new();
        if self.manual {
            reasons.push("by hand".to_string());
        }
        if self.covered {
            reasons.push("desktop covered".to_string());
        }
        if self.battery {
            reasons.push("on battery".to_string());
        }
        if let Some(c) = self.hot {
            reasons.push(format!("too hot ({c} °C)"));
        }
        if reasons.is_empty() {
            "animating".into()
        } else {
            format!("paused: {}", reasons.join(", "))
        }
    }
}

/// The sources of stops, watched together.
pub struct Pause {
    pub stops: Stops,
    i3: Option<I3>,
    i3_path: Option<String>,
    i3_retry: Option<Instant>,
    power: Option<Power>,
    thermal: Option<Thermal>,
    next_temperature: Instant,
}

impl Pause {
    /// Connect to what the options ask for and read the current state.
    pub fn open(args: &Args, output: &Output) -> Result<Pause> {
        let thermal = match args.max_temp {
            Some(max) => Some(Thermal::open(max).map_err(|e| format!("--max-temp: {e}"))?),
            None => None,
        };
        let i3_path = std::env::var("I3SOCK")
            .ok()
            .filter(|p| !p.is_empty())
            .or_else(|| output.i3_socket_path());
        let mut i3 = None;
        let mut i3_retry = None;
        if !args.ignore_covered {
            match &i3_path {
                Some(path) => match I3::connect(path) {
                    Ok(connection) => i3 = Some(connection),
                    Err(e) => {
                        eprintln!("molokolive: {e}; retrying");
                        i3_retry = Some(Instant::now() + I3_RETRY);
                    }
                },
                None => eprintln!("molokolive: i3 not found ($I3SOCK, I3_SOCKET_PATH); a covered desktop won't pause"),
            }
        }
        let power = (!args.ignore_battery).then(Power::open);
        let mut stops = Stops {
            covered: match i3.as_mut() {
                Some(connection) => !connection.desktop_visible()?,
                None => false,
            },
            battery: power.as_ref().is_some_and(Power::on_battery),
            hot: None,
            manual: false,
        };
        if let Some(t) = &thermal {
            let celsius = t.celsius()?;
            stops.hot = t.too_hot(celsius, false).then_some(celsius);
        }
        say!("{}", stops.describe());
        Ok(Pause {
            stops,
            i3,
            i3_path,
            i3_retry,
            power,
            thermal,
            next_temperature: Instant::now() + TEMPERATURE_INTERVAL,
        })
    }

    pub fn running(&self) -> bool {
        self.stops.running()
    }

    /// Descriptors that become readable when a stop may have changed.
    pub fn fds(&self) -> Vec<BorrowedFd<'_>> {
        [
            self.i3.as_ref().map(|c| c.fd()),
            self.power.as_ref().and_then(Power::fd),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// When to look again without an event: the next temperature reading, while only heat could stop the
    /// engine, or the next i3 reconnect attempt.
    pub fn deadline(&self) -> Option<Instant> {
        let checking_heat = self.thermal.is_some() && !self.stops.covered && !self.stops.battery;
        [checking_heat.then_some(self.next_temperature), self.i3_retry]
            .into_iter()
            .flatten()
            .min()
    }

    /// Take in what arrived and re-read what is due.
    pub fn poll(&mut self) -> Result<()> {
        let blocked_before = self.stops.covered || self.stops.battery;
        if let Some(connection) = self.i3.as_mut() {
            match connection.drain()? {
                Events::None => {}
                Events::Changed => self.stops.covered = !connection.desktop_visible()?,
                Events::Lost => {
                    // i3 is restarting: keep the last known state until it's back.
                    self.i3 = None;
                    self.i3_retry = Some(Instant::now() + I3_RETRY);
                }
            }
        }
        if let (None, Some(retry), Some(path)) = (&self.i3, self.i3_retry, &self.i3_path)
            && retry <= Instant::now()
        {
            match I3::connect(path) {
                Ok(mut connection) => {
                    self.stops.covered = !connection.desktop_visible()?;
                    self.i3 = Some(connection);
                    self.i3_retry = None;
                }
                Err(_) => self.i3_retry = Some(Instant::now() + I3_RETRY),
            }
        }
        if let Some(p) = &self.power
            && p.drain()
        {
            self.stops.battery = p.on_battery();
        }
        if let Some(t) = &self.thermal {
            let unblocked = blocked_before && !self.stops.covered && !self.stops.battery;
            if !self.stops.covered && !self.stops.battery && (unblocked || Instant::now() >= self.next_temperature) {
                let celsius = t.celsius()?;
                self.stops.hot = t.too_hot(celsius, self.stops.hot.is_some()).then_some(celsius);
                self.next_temperature = Instant::now() + TEMPERATURE_INTERVAL;
            }
        }
        Ok(())
    }
}
