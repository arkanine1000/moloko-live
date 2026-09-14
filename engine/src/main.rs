//! molokolive: animated scene packs (tools/pack.py) behind the desktop.
//!
//! Each frame only the canvas rectangles a timeline step changed are recomposited, converted through the palette
//! LUT at native size and uploaded; the X server then scales them onto the output in one grabbed burst (see
//! output.rs). Between steps the process sleeps in poll until the next one is due.

mod output;
mod pack;
mod render;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use output::{Output, Target};
use pack::Rect;
use render::{Fit, Geometry};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const USAGE: &str = "usage: molokolive [options]

  --scene NAME     scene pack to show (default cg_firefly)
  --packs DIR      directory of packs from tools/pack.py (default rip/packs)
  --palette NAME   palette LUT from the pack (default firefly-neutral)
  --fit MODE       cover: fill the screen, cropping overflow (default); contain: letterbox
  --output MODE    auto (default): window under a compositor, else root; window: desktop window;
                   root: root background pixmap
  --sky N          skybox still N (its game file number) instead of a random one from the pool
  --max-fps F      shortest hold is 1/F s (default 20, the game's fastest animations)
  --max-rects N    most separate draws per frame; beyond that the dirty area is covered by N strips (default 16)
  --once           set the first frame as the root background and exit
  --stats          print frame counts and per-stage timings every 10 s while animating";

struct Args {
    scene: String,
    packs: PathBuf,
    palette: String,
    fit: Fit,
    output: Target,
    sky: Option<u32>,
    max_fps: f64,
    max_rects: usize,
    once: bool,
    stats: bool,
}

/// xorshift64: hold choices and pools only need to look random.
pub struct Rng(u64);

impl Rng {
    fn seeded() -> Rng {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
        Rng(nanos | 1)
    }

    pub fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("molokolive: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let args = parse_args()?;
    let started = Instant::now();
    let mut rng = Rng::seeded();
    let dir = args.packs.join(&args.scene);
    let mut scene = pack::load(&dir, &args.palette, &mut |name, sources| choose(name, sources, args.sky, &mut rng))?;
    let loaded = started.elapsed();

    let mut output = Output::connect(args.output)?;
    let geometry = Geometry::new(scene.width, scene.height, output.width, output.height, args.fit);
    output.prepare(scene.width, scene.height, &geometry)?;
    let canvas = Rect { x0: 0, y0: 0, x1: scene.width, y1: scene.height };
    let (visible, letterbox, background) = (geometry.to_output(&canvas), geometry.letterbox(), scene.lut[scene.background as usize]);
    let mut frame = vec![scene.background; scene.width * scene.height];
    scene.composite(&mut frame, canvas);
    render::convert(&frame, scene.width, canvas, &scene.lut, output.image(0, canvas.area() * 4));
    output.upload(canvas, 0)?;
    output.publish(visible, &letterbox, background)?;
    let animated = scene.layers.iter().filter(|l| l.animation.is_some()).count();
    println!(
        "{}: canvas {}x{} -> {}x{} at x{:.3} on the {}, {} layers ({animated} animated), load {:.1} ms, first frame {:.1} ms",
        args.scene,
        scene.width,
        scene.height,
        geometry.width,
        geometry.height,
        geometry.scale,
        output.kind(),
        scene.layers.len(),
        ms(loaded),
        ms(started.elapsed() - loaded)
    );
    if args.once {
        return Ok(());
    }

    let min_hold = Duration::from_secs_f64(1.0 / args.max_fps);
    let now = Instant::now();
    for animation in scene.layers.iter_mut().filter_map(|l| l.animation.as_mut()) {
        animation.start(now, &mut rng, min_hold);
    }
    let (mut frames, mut rects, mut pixels, mut report) = (0u64, 0u64, 0u64, Instant::now());
    let [mut t_composite, mut t_convert, mut t_x] = [Duration::ZERO; 3];
    let (mut dirty, mut shown) = (Vec::new(), Vec::new());
    loop {
        let now = Instant::now();
        let width = scene.width;
        for layer in &mut scene.layers {
            let Some(animation) = layer.animation.as_mut() else { continue };
            while animation.due.is_some_and(|due| due <= now) {
                animation.advance(&mut layer.pixels, width, now, &mut rng, min_hold, &mut dirty);
            }
        }
        if !dirty.is_empty() {
            // Upload every changed rectangle into the source first (nothing visible yet), then scale them all onto
            // the screen at once.
            let mut offset = 0;
            for rect in merge(&dirty, args.max_rects) {
                let t0 = Instant::now();
                scene.composite(&mut frame, rect);
                let len = rect.area() * 4;
                if offset + len > output.capacity() {
                    output.sync()?; // the server has read what we uploaded so far
                    offset = 0;
                }
                let t1 = Instant::now();
                render::convert(&frame, scene.width, rect, &scene.lut, output.image(offset, len));
                let t2 = Instant::now();
                output.upload(rect, offset)?;
                shown.extend(geometry.to_output(&rect));
                (t_composite, t_convert, t_x) = (t_composite + (t1 - t0), t_convert + (t2 - t1), t_x + t2.elapsed());
                offset += len;
                rects += 1;
                pixels += rect.area() as u64;
            }
            let t3 = Instant::now();
            output.show_frame(&shown)?;
            output.sync()?;
            t_x += t3.elapsed();
            dirty.clear();
            shown.clear();
            frames += 1;
            if args.stats && report.elapsed() >= Duration::from_secs(10) {
                let secs = report.elapsed().as_secs_f64();
                let per_frame = |d: Duration| ms(d) / frames as f64;
                println!(
                    "{:.1} frames/s, {:.1} rects/frame, {:.0} canvas px/s; per frame: composite {:.2} ms, convert {:.2} ms, X {:.2} ms",
                    frames as f64 / secs,
                    rects as f64 / frames as f64,
                    pixels as f64 / secs,
                    per_frame(t_composite),
                    per_frame(t_convert),
                    per_frame(t_x)
                );
                (frames, rects, pixels, report) = (0, 0, 0, Instant::now());
                [t_composite, t_convert, t_x] = [Duration::ZERO; 3];
            }
        }
        let exposed = output.drain_events()?;
        if !exposed.is_empty() {
            for area in exposed {
                let bands: Vec<Rect> = letterbox.iter().filter_map(|b| b.intersect(&area)).collect();
                output.paint(visible.and_then(|v| v.intersect(&area)), &bands, background)?;
            }
            output.sync()?;
        }
        let next = scene.layers.iter().filter_map(|l| l.animation.as_ref()?.due).min();
        output.wait(next.map(|due| due.saturating_duration_since(Instant::now())))?;
    }
}

/// Merge rectangles whose union costs no more pixels than drawing them apart. Past `max` draws, each separate
/// upload and scaled composite costs more than a few extra pixels: the dirty area is then covered by `max`
/// non-overlapping horizontal strips, each spanning the dirty rectangles that cross it.
fn merge(rects: &[Rect], max: usize) -> Vec<Rect> {
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
    let bounds = merged[1..].iter().fold(merged[0], |a, r| a.union(r));
    let height = bounds.height();
    (0..max)
        .filter_map(|i| {
            let strip = Rect { y0: bounds.y0 + height * i / max, y1: bounds.y0 + height * (i + 1) / max, ..bounds };
            merged.iter().filter_map(|r| r.intersect(&strip)).reduce(|a, b| a.union(&b))
        })
        .collect()
}

/// A random image of the layer, or with `--sky N` the sky layer's still whose game file is N.png.
fn choose(name: &str, sources: &[String], sky: Option<u32>, rng: &mut Rng) -> Result<usize> {
    if let (Some(n), "sky") = (sky, name) {
        let file = format!("{n}.png");
        return sources
            .iter()
            .position(|s| s.rsplit('/').next() == Some(file.as_str()))
            .ok_or_else(|| format!("--sky {n}: not in this scene's pool").into());
    }
    Ok(rng.below(sources.len()))
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        scene: "cg_firefly".into(),
        packs: "rip/packs".into(),
        palette: "firefly-neutral".into(),
        fit: Fit::Cover,
        output: Target::Auto,
        sky: None,
        max_fps: 20.0,
        max_rects: 16,
        once: false,
        stats: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{arg} needs a value\n{USAGE}"));
        match arg.as_str() {
            "--scene" => args.scene = value()?,
            "--packs" => args.packs = value()?.into(),
            "--palette" => args.palette = value()?,
            "--fit" => {
                args.fit = match value()?.as_str() {
                    "cover" => Fit::Cover,
                    "contain" => Fit::Contain,
                    other => return Err(format!("--fit {other}: expected cover or contain").into()),
                }
            }
            "--output" => {
                args.output = match value()?.as_str() {
                    "auto" => Target::Auto,
                    "root" => Target::Root,
                    "window" => Target::Window,
                    other => return Err(format!("--output {other}: expected auto, root or window").into()),
                }
            }
            "--sky" => args.sky = Some(value()?.parse()?),
            "--max-fps" => args.max_fps = value()?.parse::<f64>()?.max(0.1),
            "--max-rects" => args.max_rects = value()?.parse::<usize>()?.max(1),
            "--once" => args.once = true,
            "--stats" => args.stats = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {arg:?}\n{USAGE}").into()),
        }
    }
    if args.once {
        // A window disappears with the process; only the root background outlives it.
        if args.output == Target::Window {
            return Err("--once needs the root background (--output root or auto)".into());
        }
        args.output = Target::Root;
    }
    Ok(args)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
