//! Scene packs written by molokolive-pack (manifest.json, version 1), and their playback state.

use std::collections::HashMap;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::{Result, Rng};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl Rect {
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
    pub name: String,
    images: Vec<PathBuf>,
    sources: Vec<String>,
    pub current: usize,
    /// The current image unshifted; the layer's pixels are this at `offset`.
    base: Vec<u8>,
    offset: (i32, i32),
    drift: Option<Drift>,
}

/// A repeating drift: whole native-pixel offsets, each from its time within the period on.
pub struct Drift {
    period: f64,
    steps: Vec<(f64, i32, i32)>,
    next: usize,
    due: Option<Instant>,
}

pub struct Animation {
    steps: Vec<Step>,
    loop_to: Option<usize>,
    wrap: Option<Patch>,
    current: usize,
    /// When the next step is entered; None once a non-looping animation has ended.
    pub due: Option<Instant>,
}

struct Step {
    holds: Vec<f64>,
    patch: Option<Patch>,
}

struct Patch {
    rect: Rect,
    pixels: Vec<u8>,
    /// Tile-aligned rectangles covering the pixels that actually change; the patch image spans their bounding box.
    dirty: Vec<Rect>,
}

#[derive(Deserialize)]
struct Manifest {
    version: u32,
    size: [usize; 2],
    background: u8,
    palettes: HashMap<String, String>,
    layers: Vec<LayerSpec>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LayerSpec {
    Choices {
        name: String,
        choices: Vec<String>,
        sources: Vec<String>,
        #[serde(default)]
        drift: Option<DriftSpec>,
    },
    Timeline {
        name: String,
        base: String,
        steps: Vec<StepSpec>,
        #[serde(rename = "loop")]
        loop_to: Option<usize>,
        wrap: Option<PatchSpec>,
    },
}

#[derive(Deserialize)]
struct DriftSpec {
    period: f64,
    steps: Vec<(f64, i32, i32)>,
}

#[derive(Deserialize)]
struct StepSpec {
    hold: Vec<f64>,
    patch: Option<PatchSpec>,
}

#[derive(Deserialize)]
struct PatchSpec {
    rect: [usize; 4],
    image: String,
    dirty: Vec<[usize; 4]>,
}

/// Load a pack with one palette. `pick(layer name, game source paths)` chooses the image of each choice layer.
pub fn load(dir: &Path, palette: &str, pick: &mut dyn FnMut(&str, &[String]) -> Result<usize>) -> Result<Scene> {
    let path = dir.join("manifest.json");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e} (run molokolive-pack)", path.display()))?;
    let manifest: Manifest = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if manifest.version != 1 {
        return Err(format!(
            "{}: version {}, expected 1 (rebuild with molokolive-pack)",
            path.display(),
            manifest.version
        )
        .into());
    }
    let [width, height] = manifest.size;

    let lut_file = manifest.palettes.get(palette).ok_or_else(|| {
        let mut names: Vec<&str> = manifest.palettes.keys().map(String::as_str).collect();
        names.sort();
        format!(
            "no palette {palette:?} in {} (available: {})",
            dir.display(),
            names.join(", ")
        )
    })?;
    let bytes = fs::read(dir.join(lut_file))?;
    let mut lut = [[0u8; 4]; 256];
    if bytes.len() != lut.len() * 4 {
        return Err(format!("{lut_file}: {} bytes, expected 1024", bytes.len()).into());
    }
    for (entry, chunk) in lut.iter_mut().zip(bytes.chunks_exact(4)) {
        entry.copy_from_slice(chunk);
    }

    let rect = |r: [usize; 4], within: Rect, what: &str| -> Result<Rect> {
        let [x0, y0, x1, y1] = r;
        if x0 >= x1 || y0 >= y1 || x0 < within.x0 || y0 < within.y0 || x1 > within.x1 || y1 > within.y1 {
            return Err(format!("{what}: rect {r:?} outside {within:?}").into());
        }
        Ok(Rect { x0, y0, x1, y1 })
    };
    let canvas = Rect {
        x0: 0,
        y0: 0,
        x1: width,
        y1: height,
    };

    let mut layers = Vec::new();
    for spec in manifest.layers {
        layers.push(match spec {
            LayerSpec::Choices {
                name,
                choices,
                sources,
                drift,
            } => {
                if choices.is_empty() || sources.len() != choices.len() {
                    return Err(format!("layer {name}: choices and sources don't match").into());
                }
                let current = pick(&name, &sources)?;
                if current >= choices.len() {
                    return Err(format!("layer {name}: no image {current}").into());
                }
                let drift = match drift {
                    Some(d)
                        if d.period > 0.0
                            && !d.steps.is_empty()
                            && d.steps.iter().all(|s| (0.0..d.period).contains(&s.0)) =>
                    {
                        Some(Drift {
                            period: d.period,
                            steps: d.steps,
                            next: 0,
                            due: None,
                        })
                    }
                    Some(_) => return Err(format!("layer {name}: bad drift").into()),
                    None => None,
                };
                let images: Vec<PathBuf> = choices.iter().map(|c| dir.join(c)).collect();
                let base = read_indexed(&images[current], width, height)?;
                // A drifting layer rests where its cycle ends, so the first frame already matches the drift.
                let offset = drift
                    .as_ref()
                    .and_then(|d| d.steps.last())
                    .map_or((0, 0), |&(_, dx, dy)| (dx, dy));
                let pixels = shifted(&base, width, height, offset);
                let choice = Choice {
                    name,
                    images,
                    sources,
                    current,
                    base,
                    offset,
                    drift,
                };
                Layer {
                    pixels,
                    animation: None,
                    choice: Some(choice),
                }
            }
            LayerSpec::Timeline {
                name,
                base,
                steps,
                loop_to,
                wrap,
            } => {
                if steps.len() < 2 || loop_to.is_some_and(|l| l >= steps.len()) {
                    return Err(format!("layer {name}: bad timeline").into());
                }
                let patch = |p: Option<PatchSpec>| -> Result<Option<Patch>> {
                    let Some(p) = p else { return Ok(None) };
                    let bounds = rect(p.rect, canvas, &p.image)?;
                    // Tile-aligned, so they may reach past the patch's exact bounds, never past the canvas.
                    let dirty = p
                        .dirty
                        .iter()
                        .map(|&d| rect(d, canvas, &p.image))
                        .collect::<Result<Vec<_>>>()?;
                    let pixels = read_indexed(&dir.join(&p.image), bounds.width(), bounds.height())?;
                    Ok(Some(Patch {
                        rect: bounds,
                        pixels,
                        dirty,
                    }))
                };
                let steps = steps
                    .into_iter()
                    .map(|s| {
                        Ok(Step {
                            holds: s.hold,
                            patch: patch(s.patch)?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                if steps.iter().any(|s| s.holds.is_empty()) {
                    return Err(format!("layer {name}: step without holds").into());
                }
                let animation = Animation {
                    steps,
                    loop_to,
                    wrap: patch(wrap)?,
                    current: 0,
                    due: None,
                };
                Layer {
                    pixels: read_indexed(&dir.join(base), width, height)?,
                    animation: Some(animation),
                    choice: None,
                }
            }
        });
    }
    Ok(Scene {
        width,
        height,
        background: manifest.background,
        lut,
        layers,
    })
}

impl Scene {
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
        // Keep the rhythm from the due time, but don't replay a backlog after a stall or suspend.
        let from = if now.saturating_duration_since(due) > Duration::from_secs(1) {
            now
        } else {
            due
        };
        self.due = Some(from + hold(&self.steps[next].holds, rng, min_hold));

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
    fn gap_before(&self, i: usize) -> f64 {
        let previous = (i + self.steps.len() - 1) % self.steps.len();
        let gap = (self.steps[i].0 - self.steps[previous].0).rem_euclid(self.period);
        if gap > 0.0 { gap } else { self.period }
    }

    /// The next offset, if its step is due by `now`.
    fn advance(&mut self, now: Instant) -> Option<(i32, i32)> {
        let due = self.due.filter(|&due| due <= now)?;
        let (_, dx, dy) = self.steps[self.next];
        self.next = (self.next + 1) % self.steps.len();
        // Keep the rhythm from the due time, but don't replay a backlog after a stall or suspend.
        let from = if now.saturating_duration_since(due) > Duration::from_secs(1) {
            now
        } else {
            due
        };
        self.due = Some(from + Duration::from_secs_f64(self.gap_before(self.next)));
        Some((dx, dy))
    }
}

/// `base` moved by (dx, dy) native pixels; the edges repeat into the uncovered strip.
fn shifted(base: &[u8], width: usize, height: usize, (dx, dy): (i32, i32)) -> Vec<u8> {
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

fn hold(holds: &[f64], rng: &mut Rng, min_hold: Duration) -> Duration {
    Duration::from_secs_f64(holds[rng.below(holds.len())]).max(min_hold)
}

fn read_indexed(path: &Path, width: usize, height: usize) -> Result<Vec<u8>> {
    let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::IDENTITY);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("PNG too large")?];
    let info = reader.next_frame(&mut buf)?;
    if info.color_type != png::ColorType::Indexed
        || info.bit_depth != png::BitDepth::Eight
        || (info.width as usize, info.height as usize) != (width, height)
    {
        return Err(format!("{}: expected an 8-bit indexed {width}x{height} PNG", path.display()).into());
    }
    buf.truncate(width * height);
    Ok(buf)
}
