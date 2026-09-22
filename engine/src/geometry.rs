//! Rectangles, and how the canvas maps onto the screen.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl Rect {
    /// The rectangle from the origin to (width, height).
    pub fn sized(width: usize, height: usize) -> Rect {
        Rect {
            x0: 0,
            y0: 0,
            x1: width,
            y1: height,
        }
    }

    pub fn width(&self) -> usize {
        self.x1 - self.x0
    }

    pub fn height(&self) -> usize {
        self.y1 - self.y0
    }

    pub fn area(&self) -> usize {
        self.width() * self.height()
    }

    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let r = Rect {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        };
        (r.x0 < r.x1 && r.y0 < r.y1).then_some(r)
    }

    pub fn union(&self, other: &Rect) -> Rect {
        Rect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// Fill the screen, cropping the canvas edges that overflow.
    Cover,
    /// Show the whole canvas, with the background around it.
    Contain,
}

/// Where each screen pixel samples the canvas. The X server scales with the same mapping (see output.rs): output
/// pixel centre -> floor((o + 0.5 - offset) / scale).
#[derive(Debug)]
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
            canvas: Rect::sized(canvas_width, canvas_height),
        }
    }

    fn screen(&self) -> Rect {
        Rect::sized(self.width, self.height)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_intersect_and_union() {
        let a = Rect {
            x0: 0,
            y0: 0,
            x1: 10,
            y1: 10,
        };
        let b = Rect {
            x0: 5,
            y0: 5,
            x1: 20,
            y1: 20,
        };
        assert_eq!(
            a.intersect(&b),
            Some(Rect {
                x0: 5,
                y0: 5,
                x1: 10,
                y1: 10
            })
        );
        assert_eq!(
            a.union(&b),
            Rect {
                x0: 0,
                y0: 0,
                x1: 20,
                y1: 20
            }
        );
        assert_eq!(
            a.intersect(&Rect {
                x0: 10,
                y0: 0,
                x1: 20,
                y1: 10
            }),
            None
        );
        assert_eq!(Rect::sized(4, 3).area(), 12);
    }

    #[test]
    fn cover_scales_to_the_larger_axis_and_crops() {
        let g = Geometry::new(100, 100, 200, 100, Fit::Cover);
        assert_eq!(g.scale, 2.0);
        assert_eq!(g.offset, (0.0, -50.0));
        assert_eq!(g.to_output(&Rect::sized(100, 100)), Some(Rect::sized(200, 100)));
        assert_eq!(
            g.to_output(&Rect {
                x0: 0,
                y0: 0,
                x1: 100,
                y1: 20
            }),
            None
        );
        assert!(g.letterbox().is_empty());
    }

    #[test]
    fn contain_leaves_bands() {
        let g = Geometry::new(100, 100, 200, 100, Fit::Contain);
        assert_eq!(g.scale, 1.0);
        assert_eq!(
            g.to_output(&Rect::sized(100, 100)),
            Some(Rect {
                x0: 50,
                y0: 0,
                x1: 150,
                y1: 100
            })
        );
        assert_eq!(
            g.letterbox(),
            vec![
                Rect {
                    x0: 0,
                    y0: 0,
                    x1: 50,
                    y1: 100
                },
                Rect {
                    x0: 150,
                    y0: 0,
                    x1: 200,
                    y1: 100
                }
            ]
        );
    }

    #[test]
    fn output_pixels_sample_their_centre() {
        let g = Geometry::new(4, 4, 6, 6, Fit::Cover);
        assert_eq!(
            g.to_output(&Rect {
                x0: 1,
                y0: 1,
                x1: 2,
                y1: 2
            }),
            Some(Rect {
                x0: 1,
                y0: 1,
                x1: 3,
                y1: 3
            })
        );
    }
}
