//! A loaded scene: its layers, the image each pool shows, and compositing.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::manifest::read_indexed;
use super::timeline::{Animation, Drift};
use crate::Result;
use crate::geometry::Rect;

pub struct Scene {
    pub width: usize,
    pub height: usize,
    pub background: u8,
    pub lut: [[u8; 4]; 256],
    pub layers: Vec<Layer>,
}

pub struct Layer {
    /// The layer's current image, canvas-sized; index 0 is transparent.
    pub pixels: Vec<u8>,
    pub animation: Option<Animation>,
    pub choice: Option<Choice>,
}

/// A layer showing one of several images (a sky or reflection pool), possibly drifting.
pub struct Choice {
    pub(super) name: String,
    pub(super) images: Vec<PathBuf>,
    pub(super) sources: Vec<String>,
    pub(super) current: usize,
    /// The current image unshifted; the layer's pixels are this at `offset`.
    pub(super) base: Vec<u8>,
    pub(super) offset: (i32, i32),
    pub(super) drift: Option<Drift>,
}

impl Scene {
    pub fn canvas(&self) -> Rect {
        Rect::sized(self.width, self.height)
    }

    /// Move choice layer `name` by `step` images (wrapping) -> its new index, or None if the scene has no such layer.
    /// The caller redraws the canvas.
    pub fn step_choice(&mut self, name: &str, step: isize) -> Result<Option<usize>> {
        let (width, height) = (self.width, self.height);
        let Some(layer) = self
            .layers
            .iter_mut()
            .find(|l| l.choice.as_ref().is_some_and(|c| c.name == name))
        else {
            return Ok(None);
        };
        let Some(choice) = layer.choice.as_mut() else {
            return Ok(None);
        };
        let index = (choice.current as isize + step).rem_euclid(choice.images.len() as isize) as usize;
        choice.base = read_indexed(&choice.images[index], width, height)?;
        layer.pixels = shifted(&choice.base, width, height, choice.offset);
        choice.current = index;
        Ok(Some(index))
    }

    /// Start every drift at the beginning of its cycle (the layers already rest at its end offset).
    pub fn start_drift(&mut self, now: Instant) {
        for drift in self.drifts() {
            drift.next = 0;
            drift.due = Some(now + Duration::from_secs_f64(drift.steps[0].0.max(0.001)));
        }
    }

    /// Continue drifting after a pause: the next offset comes one step's interval from now.
    pub fn resume_drift(&mut self, now: Instant) {
        for drift in self.drifts() {
            if drift.due.is_some() {
                drift.due = Some(now + Duration::from_secs_f64(drift.gap_before(drift.next)));
            }
        }
    }

    /// When the next drift step is due.
    pub fn drift_due(&self) -> Option<Instant> {
        self.layers
            .iter()
            .filter_map(|l| l.choice.as_ref()?.drift.as_ref()?.due)
            .min()
    }

    /// When the next animation step is due.
    pub fn animation_due(&self) -> Option<Instant> {
        self.layers.iter().filter_map(|l| l.animation.as_ref()?.due).min()
    }

    /// Take every drift step due by `now`; whether any layer moved.
    pub fn advance_drift(&mut self, now: Instant) -> bool {
        let (width, height) = (self.width, self.height);
        let mut moved = false;
        for layer in &mut self.layers {
            let Some(choice) = layer.choice.as_mut() else { continue };
            let Some(drift) = choice.drift.as_mut() else { continue };
            while let Some(offset) = drift.advance(now) {
                if offset != choice.offset {
                    choice.offset = offset;
                    layer.pixels = shifted(&choice.base, width, height, offset);
                    moved = true;
                }
            }
        }
        moved
    }

    fn drifts(&mut self) -> impl Iterator<Item = &mut Drift> {
        self.layers.iter_mut().filter_map(|l| l.choice.as_mut()?.drift.as_mut())
    }

    /// (layer name, image index) of every choice layer.
    pub fn choices(&self) -> Vec<(String, usize)> {
        self.layers
            .iter()
            .filter_map(|l| l.choice.as_ref())
            .map(|c| (c.name.clone(), c.current))
            .collect()
    }

    /// The images picked from pools, e.g. "sky 20, reflection 84" (game file numbers).
    pub fn picks(&self) -> String {
        self.layers
            .iter()
            .filter_map(|l| l.choice.as_ref())
            .filter(|c| c.images.len() > 1)
            .map(|c| {
                let file = Path::new(&c.sources[c.current])
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned());
                format!("{} {}", c.name, file.unwrap_or_default())
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Resolve the topmost opaque index of every pixel in `rect` into `frame`: the bottom layer over the background,
    /// then each layer above over that. Branch-free selects per pixel, so the loops vectorise.
    pub fn composite(&self, frame: &mut [u8], rect: Rect) {
        let Some((bottom, above)) = self.layers.split_first() else {
            return;
        };
        let background = self.background;
        for y in rect.y0..rect.y1 {
            let span = y * self.width + rect.x0..y * self.width + rect.x1;
            let dst = &mut frame[span.clone()];
            for (d, &s) in dst.iter_mut().zip(&bottom.pixels[span.clone()]) {
                *d = if s != 0 { s } else { background };
            }
            for layer in above {
                for (d, &s) in dst.iter_mut().zip(&layer.pixels[span.clone()]) {
                    *d = if s != 0 { s } else { *d };
                }
            }
        }
    }
}

/// `base` moved by (dx, dy) native pixels; the edges repeat into the uncovered strip.
pub(super) fn shifted(base: &[u8], width: usize, height: usize, (dx, dy): (i32, i32)) -> Vec<u8> {
    if (dx, dy) == (0, 0) {
        return base.to_vec();
    }
    let mut out = vec![0; base.len()];
    for (y, row) in out.chunks_exact_mut(width).enumerate() {
        let sy = (y as i32 - dy).clamp(0, height as i32 - 1) as usize;
        let source = &base[sy * width..][..width];
        for (x, px) in row.iter_mut().enumerate() {
            *px = source[(x as i32 - dx).clamp(0, width as i32 - 1) as usize];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifted_repeats_the_edges() {
        let base = [1, 2, 3, 4, 5, 6, 7, 8, 9];
        assert_eq!(shifted(&base, 3, 3, (1, 0)), [1, 1, 2, 4, 4, 5, 7, 7, 8]);
        assert_eq!(shifted(&base, 3, 3, (0, -1)), [4, 5, 6, 7, 8, 9, 7, 8, 9]);
        assert_eq!(shifted(&base, 3, 3, (0, 0)), base);
    }

    #[test]
    fn composite_takes_the_topmost_opaque_index() {
        let layer = |pixels: Vec<u8>| Layer {
            pixels,
            animation: None,
            choice: None,
        };
        let scene = Scene {
            width: 2,
            height: 2,
            background: 9,
            lut: [[0; 4]; 256],
            layers: vec![layer(vec![1, 0, 1, 0]), layer(vec![0, 0, 2, 2])],
        };
        let mut frame = vec![0; 4];
        scene.composite(&mut frame, scene.canvas());
        assert_eq!(frame, [1, 9, 2, 2]);
        frame = vec![7; 4];
        scene.composite(
            &mut frame,
            Rect {
                x0: 1,
                y0: 1,
                x1: 2,
                y1: 2,
            },
        );
        assert_eq!(frame, [7, 7, 7, 2]);
    }
}
