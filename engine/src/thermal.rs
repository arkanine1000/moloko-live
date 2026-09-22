//! The CPU package temperature, read from sysfs. There are no events for it, so it is polled.

use std::fs;
use std::path::PathBuf;

use crate::Result;

pub struct Thermal {
    path: PathBuf,
    /// Stop at or above this temperature (°C).
    max: i32,
}

impl Thermal {
    /// Resume only once the temperature is this many degrees below the maximum.
    const HYSTERESIS: i32 = 5;

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
        Err(format!("no x86_pkg_temp thermal zone (found: {})", types.join(", ")).into())
    }

    pub fn celsius(&self) -> Result<i32> {
        let millidegrees: i32 = fs::read_to_string(&self.path)?.trim().parse()?;
        Ok(millidegrees / 1000)
    }

    /// Whether to be stopped at `celsius`, given whether the engine already is.
    pub fn too_hot(&self, celsius: i32, stopped: bool) -> bool {
        too_hot(self.max, celsius, stopped)
    }
}

fn too_hot(max: i32, celsius: i32, stopped: bool) -> bool {
    if stopped {
        celsius > max - Thermal::HYSTERESIS
    } else {
        celsius >= max
    }
}

#[cfg(test)]
mod tests {
    use super::too_hot;

    #[test]
    fn stops_at_the_limit_and_resumes_below_the_hysteresis() {
        assert!(!too_hot(80, 79, false));
        assert!(too_hot(80, 80, false));
        assert!(too_hot(80, 76, true));
        assert!(!too_hot(80, 75, true));
    }
}
