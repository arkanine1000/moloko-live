//! The files of a pack: manifest.json, indexed PNG images and palette LUTs.

use std::collections::HashMap;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::scene::{Choice, Layer, Scene, shifted};
use super::timeline::{Animation, Drift, Patch, Step};
use crate::Result;
use crate::geometry::Rect;

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

fn read_manifest(dir: &Path) -> Result<Manifest> {
    let path = dir.join("manifest.json");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let manifest: Manifest = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if manifest.version != 1 {
        return Err(format!("{}: version {}, expected 1", path.display(), manifest.version).into());
    }
    Ok(manifest)
}

/// What a pack has, for listings.
#[derive(Debug, PartialEq, Eq)]
pub struct Summary {
    pub sky: bool,
    pub reflection: bool,
    pub animated: bool,
}

pub fn summary(dir: &Path) -> Result<Summary> {
    let manifest = read_manifest(dir)?;
    let pool = |layer: &str| {
        manifest
            .layers
            .iter()
            .any(|l| matches!(l, LayerSpec::Choices { name, choices, .. } if name == layer && choices.len() > 1))
    };
    let animated = manifest.layers.iter().any(|l| matches!(l, LayerSpec::Timeline { .. }));
    Ok(Summary {
        sky: pool("sky"),
        reflection: pool("reflection"),
        animated,
    })
}

/// Load a pack with one palette. `pick(layer name, game source paths)` chooses the image of each choice layer.
pub fn load(dir: &Path, palette: &str, pick: &mut dyn FnMut(&str, &[String]) -> Result<usize>) -> Result<Scene> {
    let manifest = read_manifest(dir)?;
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
        return Err(format!("{lut_file}: {} bytes, expected {}", bytes.len(), lut.len() * 4).into());
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
    let canvas = Rect::sized(width, height);

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

pub(super) fn read_indexed(path: &Path, width: usize, height: usize) -> Result<Vec<u8>> {
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
