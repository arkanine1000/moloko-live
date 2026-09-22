//! The scene on screen and everything derived from it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::cli::Args;
use crate::geometry::{Geometry, Rect};
use crate::output::Output;
use crate::rotation::Rng;
use crate::{Result, pack, render};

/// The image each pool layer showed last, by (scene, layer), for --persist-sky.
pub type Picks = HashMap<(String, String), usize>;

pub struct Show {
    pub name: String,
    pub scene: pack::Scene,
    pub geometry: Geometry,
    /// The composited canvas as it is on screen.
    pub frame: Vec<u8>,
    /// Where the canvas lands on the screen, and the screen areas around it.
    pub visible: Option<Rect>,
    pub letterbox: Vec<Rect>,
    pub background: [u8; 4],
    /// A full recomposite after a drift step, to compare with `frame`.
    scratch: Vec<u8>,
}

impl Show {
    /// Load a scene, picking each pool's image: `sky` for the sky layer if given, the scene's remembered image with
    /// --persist-sky, else a random one. Records the picks. Doesn't touch the output.
    pub fn open(
        args: &Args,
        name: &str,
        output: &Output,
        rng: &mut Rng,
        picks: &mut Picks,
        sky: Option<u32>,
    ) -> Result<Show> {
        let mut pick = |layer: &str, sources: &[String]| -> Result<usize> {
            if let (Some(n), "sky") = (sky, layer) {
                let file = format!("{n}.png");
                return sources
                    .iter()
                    .position(|s| s.rsplit('/').next() == Some(file.as_str()))
                    .ok_or_else(|| format!("sky {n} isn't one of {name}'s skies").into());
            }
            if args.persist_sky
                && let Some(&index) = picks.get(&(name.to_string(), layer.to_string()))
                && index < sources.len()
            {
                return Ok(index);
            }
            Ok(rng.below(sources.len()))
        };
        let scene = pack::load(&args.packs.join(name), &args.palette, &mut pick)?;
        for (layer, index) in scene.choices() {
            picks.insert((name.to_string(), layer), index);
        }
        let geometry = Geometry::new(scene.width, scene.height, output.width, output.height, args.fit);
        let canvas = scene.canvas();
        let mut frame = vec![scene.background; scene.width * scene.height];
        scene.composite(&mut frame, canvas);
        Ok(Show {
            name: name.to_string(),
            visible: geometry.to_output(&canvas),
            letterbox: geometry.letterbox(),
            background: scene.lut[scene.background as usize],
            scratch: Vec::new(),
            geometry,
            frame,
            scene,
        })
    }

    /// Give the output a source picture for this scene and upload the whole canvas into it.
    pub fn upload(&self, output: &mut Output) -> Result<()> {
        output.prepare(self.scene.width, self.scene.height, &self.geometry)?;
        let canvas = self.scene.canvas();
        render::convert(
            &self.frame,
            self.scene.width,
            canvas,
            &self.scene.lut,
            output.image(0, canvas.area() * 4),
        );
        output.upload(canvas, 0)
    }

    pub fn start(&mut self, rng: &mut Rng, min_hold: Duration, drift: bool) {
        let now = Instant::now();
        for animation in self.scene.layers.iter_mut().filter_map(|l| l.animation.as_mut()) {
            animation.start(now, rng, min_hold);
        }
        if drift {
            self.scene.start_drift(now);
        }
    }

    /// Continue after a pause, without replaying what was missed.
    pub fn resume(&mut self, rng: &mut Rng, min_hold: Duration) {
        let now = Instant::now();
        for animation in self.scene.layers.iter_mut().filter_map(|l| l.animation.as_mut()) {
            animation.resume(now, rng, min_hold);
        }
        self.scene.resume_drift(now);
    }

    /// Canvas areas where the layers now differ from the frame on screen, as runs of tiles. A drift step moves a
    /// whole layer, but only pixels where it shows and differs from its neighbour change.
    pub fn changed_tiles(&mut self, dirty: &mut Vec<Rect>) {
        const TILE: usize = 16;
        let (width, height) = (self.scene.width, self.scene.height);
        let canvas = self.scene.canvas();
        self.scratch.resize(width * height, 0);
        self.scene.composite(&mut self.scratch, canvas);
        for y0 in (0..height).step_by(TILE) {
            let y1 = (y0 + TILE).min(height);
            let mut run = None;
            for x0 in (0..width).step_by(TILE) {
                let x1 = (x0 + TILE).min(width);
                let changed = (y0..y1).any(|y| {
                    self.scratch[y * width + x0..y * width + x1] != self.frame[y * width + x0..y * width + x1]
                });
                match (changed, run) {
                    (true, None) => run = Some(x0),
                    (false, Some(start)) => {
                        dirty.push(Rect {
                            x0: start,
                            y0,
                            x1: x0,
                            y1,
                        });
                        run = None;
                    }
                    _ => {}
                }
            }
            if let Some(start) = run {
                dirty.push(Rect {
                    x0: start,
                    y0,
                    x1: width,
                    y1,
                });
            }
        }
    }

    /// The scene and its picks, e.g. "cg_floor (sky 20)".
    pub fn describe(&self) -> String {
        let picks = self.scene.picks();
        if picks.is_empty() {
            self.name.clone()
        } else {
            format!("{} ({picks})", self.name)
        }
    }
}
