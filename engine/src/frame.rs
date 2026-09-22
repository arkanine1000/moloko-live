//! What gets drawn per frame: the dirty rectangles merged into few draws, and statistics about them.

use std::time::{Duration, Instant};

use crate::geometry::Rect;
use crate::say;

/// Draw limit for frames with a drift step: their changes are thin and spread over the whole canvas, so merging
/// them into a few strips would repaint most of the screen.
pub const DRIFT_MAX_RECTS: usize = 256;

/// Merge rectangles whose union costs no more pixels than drawing them apart, and past `max` of them cover the
/// dirty area with `max` horizontal strips instead.
pub fn merge(rects: &[Rect], max: usize) -> Vec<Rect> {
    let mut merged: Vec<Rect> = Vec::new();
    for r in rects {
        match merged.iter_mut().find(|m| m.union(r).area() <= m.area() + r.area()) {
            Some(m) => *m = m.union(r),
            None => merged.push(*r),
        }
    }
    if merged.len() <= max {
        return merged;
    }
    // Each separate upload and scaled composite costs more than a few extra pixels: a strip spans the dirty
    // rectangles that cross it.
    let bounds = merged[1..].iter().fold(merged[0], |a, r| a.union(r));
    let height = bounds.height();
    (0..max)
        .filter_map(|i| {
            let strip = Rect {
                y0: bounds.y0 + height * i / max,
                y1: bounds.y0 + height * (i + 1) / max,
                ..bounds
            };
            merged
                .iter()
                .filter_map(|r| r.intersect(&strip))
                .reduce(|a, b| a.union(&b))
        })
        .collect()
}

/// Drawing statistics, printed every `REPORT_INTERVAL` with --stats.
pub struct Stats {
    frames: u64,
    rects: u64,
    pixels: u64,
    composite: Duration,
    convert: Duration,
    x: Duration,
    since: Instant,
}

impl Stats {
    const REPORT_INTERVAL: Duration = Duration::from_secs(10);

    pub fn new() -> Stats {
        Stats {
            frames: 0,
            rects: 0,
            pixels: 0,
            composite: Duration::ZERO,
            convert: Duration::ZERO,
            x: Duration::ZERO,
            since: Instant::now(),
        }
    }

    /// Account for one drawn rectangle and the time its three stages took.
    pub fn rect(&mut self, pixels: usize, composite: Duration, convert: Duration, x: Duration) {
        self.rects += 1;
        self.pixels += pixels as u64;
        self.composite += composite;
        self.convert += convert;
        self.x += x;
    }

    /// Account for a finished frame; print and reset once the interval has passed.
    pub fn frame(&mut self, x: Duration) {
        self.frames += 1;
        self.x += x;
        if self.since.elapsed() < Stats::REPORT_INTERVAL {
            return;
        }
        let secs = self.since.elapsed().as_secs_f64();
        let per_frame = |d: Duration| ms(d) / self.frames as f64;
        say!(
            "{:.1} frames/s, {:.1} rects/frame, {:.0} canvas px/s; per frame: composite {:.2} ms, convert {:.2} ms, X {:.2} ms",
            self.frames as f64 / secs,
            self.rects as f64 / self.frames as f64,
            self.pixels as f64 / secs,
            per_frame(self.composite),
            per_frame(self.convert),
            per_frame(self.x)
        );
        *self = Stats::new();
    }
}

pub fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Rect {
        Rect { x0, y0, x1, y1 }
    }

    #[test]
    fn merges_rectangles_whose_union_is_free() {
        let adjacent = [rect(0, 0, 10, 10), rect(10, 0, 20, 10)];
        assert_eq!(merge(&adjacent, 16), vec![rect(0, 0, 20, 10)]);
        let apart = [rect(0, 0, 10, 10), rect(50, 50, 60, 60)];
        assert_eq!(merge(&apart, 16), apart.to_vec());
        assert_eq!(merge(&[], 16), vec![]);
    }

    #[test]
    fn past_the_limit_the_area_is_covered_by_strips() {
        let scattered = [
            rect(0, 0, 1, 1),
            rect(50, 10, 51, 11),
            rect(20, 20, 21, 21),
            rect(5, 39, 6, 40),
        ];
        let strips = merge(&scattered, 2);
        assert_eq!(strips, vec![rect(0, 0, 51, 11), rect(5, 20, 21, 40)]);
        for r in &scattered {
            assert!(strips.iter().any(|s| s.intersect(r) == Some(*r)));
        }
    }
}
