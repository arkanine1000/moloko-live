//! Canvas <-> screen geometry, and index -> BGRX conversion at native resolution.
//!
//! Scaling happens in the X server (a RENDER transform with the nearest filter, see output.rs), so the CPU only
//! converts changed canvas pixels and uploads them at native size. The geometry here uses the same mapping as the
//! transform, output pixel centre -> floor((o + 0.5 - offset) / scale), to know which screen areas a canvas
//! rectangle covers.

use crate::pack::Rect;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Fill the screen, cropping the canvas edges that overflow.
    Cover,
    /// Show the whole canvas, with the background around it.
    Contain,
}

pub struct Geometry {
    pub width: usize,
    pub height: usize,
    pub scale: f64,
    /// Screen position of the canvas origin (negative when cover crops it).
    pub offset: (f64, f64),
    /// Source column/row for each screen column/row; outside 0..canvas size means off the canvas.
    xmap: Vec<i32>,
    ymap: Vec<i32>,
    canvas: Rect,
}

impl Geometry {
    pub fn new(canvas_width: usize, canvas_height: usize, width: usize, height: usize, fit: Fit) -> Geometry {
        let (sx, sy) = (width as f64 / canvas_width as f64, height as f64 / canvas_height as f64);
        let scale = if fit == Fit::Cover { sx.max(sy) } else { sx.min(sy) };
        let offset = (
            (width as f64 - canvas_width as f64 * scale) / 2.0,
            (height as f64 - canvas_height as f64 * scale) / 2.0,
        );
        let axis = |out: usize, offset: f64| -> Vec<i32> {
            (0..out)
                .map(|o| ((o as f64 + 0.5 - offset) / scale).floor() as i32)
                .collect()
        };
        Geometry {
            width,
            height,
            scale,
            offset,
            xmap: axis(width, offset.0),
            ymap: axis(height, offset.1),
            canvas: Rect {
                x0: 0,
                y0: 0,
                x1: canvas_width,
                y1: canvas_height,
            },
        }
    }

    pub fn screen(&self) -> Rect {
        Rect {
            x0: 0,
            y0: 0,
            x1: self.width,
            y1: self.height,
        }
    }

    /// The screen pixels whose source lies in the canvas rectangle, or None if all of them are off screen.
    pub fn to_output(&self, r: &Rect) -> Option<Rect> {
        let span = |map: &[i32], a: usize, b: usize| {
            (
                map.partition_point(|&v| v < a as i32),
                map.partition_point(|&v| v < b as i32),
            )
        };
        let ((x0, x1), (y0, y1)) = (span(&self.xmap, r.x0, r.x1), span(&self.ymap, r.y0, r.y1));
        (x0 < x1 && y0 < y1).then_some(Rect { x0, y0, x1, y1 })
    }

    /// Screen areas outside the canvas (contain mode), to fill with the background.
    pub fn letterbox(&self) -> Vec<Rect> {
        let s = self.screen();
        let Some(c) = self.to_output(&self.canvas) else {
            return vec![s];
        };
        let bands = [
            Rect {
                x0: 0,
                y0: 0,
                x1: s.x1,
                y1: c.y0,
            },
            Rect {
                x0: 0,
                y0: c.y1,
                x1: s.x1,
                y1: s.y1,
            },
            Rect {
                x0: 0,
                y0: c.y0,
                x1: c.x0,
                y1: c.y1,
            },
            Rect {
                x0: c.x1,
                y0: c.y0,
                x1: s.x1,
                y1: c.y1,
            },
        ];
        bands.into_iter().filter(|b| b.x0 < b.x1 && b.y0 < b.y1).collect()
    }
}

/// Canvas rectangle `r` of the composited `frame` -> BGRX rows at the start of `buf`.
pub fn convert(frame: &[u8], canvas_width: usize, r: Rect, lut: &[[u8; 4]; 256], buf: &mut [u8]) {
    for (row, dst) in buf.chunks_exact_mut(r.width() * 4).take(r.height()).enumerate() {
        let src = &frame[(r.y0 + row) * canvas_width + r.x0..][..r.width()];
        for (px, &index) in dst.chunks_exact_mut(4).zip(src) {
            px.copy_from_slice(&lut[index as usize]);
        }
    }
}
