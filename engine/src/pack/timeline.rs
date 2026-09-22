//! When things happen: animation steps with random holds, and drift steps on a fixed cycle.

use std::time::{Duration, Instant};

use crate::geometry::Rect;
use crate::rotation::Rng;

/// After a stall (a suspend, a stopped process) longer than this, the backlog of steps is dropped rather than replayed.
const STALL: Duration = Duration::from_secs(1);

pub struct Animation {
    pub(super) steps: Vec<Step>,
    pub(super) loop_to: Option<usize>,
    pub(super) wrap: Option<Patch>,
    pub(super) current: usize,
    /// When the next step is entered; None once a non-looping animation has ended.
    pub due: Option<Instant>,
}

pub(super) struct Step {
    pub(super) holds: Vec<f64>,
    pub(super) patch: Option<Patch>,
}

pub(super) struct Patch {
    pub(super) rect: Rect,
    pub(super) pixels: Vec<u8>,
    /// Tile-aligned rectangles covering the pixels that actually change; the patch image spans their bounding box.
    pub(super) dirty: Vec<Rect>,
}

/// A repeating drift: whole native-pixel offsets, each from its time within the period on.
pub struct Drift {
    pub(super) period: f64,
    pub(super) steps: Vec<(f64, i32, i32)>,
    pub(super) next: usize,
    pub(super) due: Option<Instant>,
}

impl Animation {
    pub fn start(&mut self, now: Instant, rng: &mut Rng, min_hold: Duration) {
        self.current = 0;
        self.due = Some(now + hold(&self.steps[0].holds, rng, min_hold));
    }

    /// Continue after a pause: the current image stays and its hold starts over, so nothing is replayed.
    pub fn resume(&mut self, now: Instant, rng: &mut Rng, min_hold: Duration) {
        if self.due.is_some() {
            self.due = Some(now + hold(&self.steps[self.current].holds, rng, min_hold));
        }
    }

    /// Enter the next step: patch `pixels` (canvas `width` wide) and add the changed rectangles to `dirty`.
    pub fn advance(
        &mut self,
        pixels: &mut [u8],
        width: usize,
        now: Instant,
        rng: &mut Rng,
        min_hold: Duration,
        dirty: &mut Vec<Rect>,
    ) {
        let Some(due) = self.due else { return };
        let (next, patch) = if self.current + 1 < self.steps.len() {
            (self.current + 1, self.steps[self.current + 1].patch.as_ref())
        } else if let Some(target) = self.loop_to {
            (target, self.wrap.as_ref())
        } else {
            self.due = None;
            return;
        };
        self.current = next;
        self.due = Some(catch_up(now, due) + hold(&self.steps[next].holds, rng, min_hold));

        let Some(patch) = patch else { return };
        let w = patch.rect.width();
        for (row, src) in patch.pixels.chunks_exact(w).enumerate() {
            let start = (patch.rect.y0 + row) * width + patch.rect.x0;
            pixels[start..start + w].copy_from_slice(src);
        }
        dirty.extend_from_slice(&patch.dirty);
    }
}

impl Drift {
    /// Seconds from the step before `i` to step `i`.
    pub(super) fn gap_before(&self, i: usize) -> f64 {
        let previous = (i + self.steps.len() - 1) % self.steps.len();
        let gap = (self.steps[i].0 - self.steps[previous].0).rem_euclid(self.period);
        if gap > 0.0 { gap } else { self.period }
    }

    /// The next offset, if its step is due by `now`.
    pub(super) fn advance(&mut self, now: Instant) -> Option<(i32, i32)> {
        let due = self.due.filter(|&due| due <= now)?;
        let (_, dx, dy) = self.steps[self.next];
        self.next = (self.next + 1) % self.steps.len();
        self.due = Some(catch_up(now, due) + Duration::from_secs_f64(self.gap_before(self.next)));
        Some((dx, dy))
    }
}

/// Where the next interval counts from: the due time, to keep the rhythm, unless the step is so late that the
/// backlog would be replayed.
fn catch_up(now: Instant, due: Instant) -> Instant {
    if now.saturating_duration_since(due) > STALL {
        now
    } else {
        due
    }
}

/// One of the step's holds at random, no shorter than `min_hold`.
pub(super) fn hold(holds: &[f64], rng: &mut Rng, min_hold: Duration) -> Duration {
    Duration::from_secs_f64(holds[rng.below(holds.len())]).max(min_hold)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catch_up_keeps_the_rhythm_unless_stalled() {
        let due = Instant::now();
        assert_eq!(catch_up(due + Duration::from_millis(500), due), due);
        let late = due + Duration::from_secs(3);
        assert_eq!(catch_up(late, due), late);
    }

    #[test]
    fn drift_gaps_wrap_around_the_period() {
        let drift = Drift {
            period: 8.0,
            steps: vec![(1.0, 0, 0), (3.0, 0, 0), (7.0, 0, 0)],
            next: 0,
            due: None,
        };
        assert_eq!(drift.gap_before(1), 2.0);
        assert_eq!(drift.gap_before(0), 2.0);
        let single = Drift {
            period: 8.0,
            steps: vec![(1.0, 0, 0)],
            next: 0,
            due: None,
        };
        assert_eq!(single.gap_before(0), 8.0);
    }

    #[test]
    fn holds_are_floored_at_the_frame_cap() {
        let mut rng = Rng::from_seed(1);
        assert_eq!(
            hold(&[0.01], &mut rng, Duration::from_millis(50)),
            Duration::from_millis(50)
        );
        assert_eq!(
            hold(&[0.5], &mut rng, Duration::from_millis(50)),
            Duration::from_millis(500)
        );
    }
}
