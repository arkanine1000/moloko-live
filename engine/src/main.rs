//! molokolive: animated scene packs (tools/pack.py) behind the desktop.
//!
//! Each frame only the canvas rectangles a timeline step changed are recomposited, converted through the palette
//! LUT at native size and uploaded; the X server then scales them onto the output in one grabbed burst (see
//! output.rs). Between steps the process sleeps in poll until the next one is due.
//!
//! Animation runs only while the desktop can be seen (i3), the machine is on mains, and the CPU is below --max-temp.
//! Otherwise every timer stops, the last frame stays on screen, and the process waits for events.

mod i3;
mod output;
mod pack;
mod power;
mod render;

use std::os::fd::BorrowedFd;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use output::{Output, Target};
use pack::Rect;
use power::{Power, Thermal};
use render::{Fit, Geometry};
use rustix::event::{PollFd, PollFlags, Timespec};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const USAGE: &str = "usage: molokolive [options]

  --scene NAME       scene pack to show (default cg_firefly)
  --packs DIR        directory of packs from tools/pack.py (default rip/packs)
  --palette NAME     palette LUT from the pack (default neutral-lift)
  --fit MODE         cover: fill the screen, cropping overflow (default); contain: letterbox
  --output MODE      auto (default): window under a compositor, else root; window: desktop window;
                     root: root background pixmap
  --sky N            skybox still N (its game file number) instead of a random one from the pool
  --max-fps F        shortest hold is 1/F s (default 20, the game's fastest animations)
  --max-rects N      most separate draws per frame; beyond that the dirty area is covered by N strips (default 16)
  --max-temp C       stop at or above C °C (x86_pkg_temp), resume 5 °C below (default: no limit)
  --ignore-covered   keep animating when windows cover the desktop
  --ignore-battery   keep animating on battery
  --once             set the first frame as the root background and exit
  --stats            print frame counts and per-stage timings every 10 s while animating";

/// How often the temperature is read while animating, or while stopped for heat.
const TEMPERATURE_INTERVAL: Duration = Duration::from_secs(5);
/// How long to wait before reconnecting after i3 restarts.
const I3_RETRY: Duration = Duration::from_secs(2);

struct Args {
    scene: String,
    packs: PathBuf,
    palette: String,
    fit: Fit,
    output: Target,
    sky: Option<u32>,
    max_fps: f64,
    max_rects: usize,
    max_temp: Option<i32>,
    ignore_covered: bool,
    ignore_battery: bool,
    once: bool,
    stats: bool,
}

/// Why animation is stopped, if it is.
#[derive(Clone, Copy, Default)]
struct Stops {
    covered: bool,
    battery: bool,
    hot: Option<i32>,
}

impl Stops {
    fn running(&self) -> bool {
        !self.covered && !self.battery && self.hot.is_none()
    }

    fn describe(&self) -> String {
        let mut reasons = Vec::new();
        if self.covered {
            reasons.push("desktop covered".to_string());
        }
        if self.battery {
            reasons.push("on battery".to_string());
        }
        if let Some(c) = self.hot {
            reasons.push(format!("too hot ({c} °C)"));
        }
        if reasons.is_empty() { "animating".into() } else { format!("paused: {}", reasons.join(", ")) }
    }
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
    let thermal = args.max_temp.map(Thermal::open).transpose()?;
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

    // What may stop animation, and its state now.
    let i3_path = std::env::var("I3SOCK").ok().filter(|p| !p.is_empty()).or_else(|| output.i3_socket_path());
    let mut i3 = None;
    let mut i3_retry = None;
    if !args.ignore_covered {
        match &i3_path {
            Some(path) => match i3::I3::connect(path) {
                Ok(connection) => i3 = Some(connection),
                Err(e) => {
                    eprintln!("molokolive: {e}; retrying");
                    i3_retry = Some(Instant::now() + I3_RETRY);
                }
            },
            None => eprintln!("molokolive: i3 not found ($I3SOCK, I3_SOCKET_PATH); a covered desktop won't pause"),
        }
    }
    let power = (!args.ignore_battery).then(Power::open);
    let mut stops = Stops {
        covered: match i3.as_mut() {
            Some(connection) => !connection.desktop_visible()?,
            None => false,
        },
        battery: power.as_ref().is_some_and(Power::on_battery),
        hot: None,
    };
    if let Some(t) = &thermal {
        let celsius = t.celsius()?;
        stops.hot = t.too_hot(celsius, false).then_some(celsius);
    }
    let mut next_temperature = Instant::now() + TEMPERATURE_INTERVAL;
    let mut running = stops.running();
    println!("{}", stops.describe());

    let min_hold = Duration::from_secs_f64(1.0 / args.max_fps);
    let now = Instant::now();
    for animation in scene.layers.iter_mut().filter_map(|l| l.animation.as_mut()) {
        animation.start(now, &mut rng, min_hold);
    }
    let (mut frames, mut rects, mut pixels, mut report) = (0u64, 0u64, 0u64, Instant::now());
    let [mut t_composite, mut t_convert, mut t_x] = [Duration::ZERO; 3];
    let (mut dirty, mut shown) = (Vec::new(), Vec::new());
    loop {
        if running {
            let now = Instant::now();
            let width = scene.width;
            for layer in &mut scene.layers {
                let Some(animation) = layer.animation.as_mut() else { continue };
                while animation.due.is_some_and(|due| due <= now) {
                    animation.advance(&mut layer.pixels, width, now, &mut rng, min_hold, &mut dirty);
                }
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

        // Sleep until something is due: the next animation step while running, a temperature reading while only heat
        // could be stopping us, an i3 reconnect attempt. With none of those, block on events alone.
        let checking_heat = thermal.is_some() && !stops.covered && !stops.battery;
        let deadline = [
            if running { scene.layers.iter().filter_map(|l| l.animation.as_ref()?.due).min() } else { None },
            checking_heat.then_some(next_temperature),
            i3_retry,
        ]
        .into_iter()
        .flatten()
        .min();
        {
            let fds: Vec<BorrowedFd> = [Some(output.fd()), i3.as_ref().map(|c| c.fd()), power.as_ref().and_then(Power::fd)]
                .into_iter()
                .flatten()
                .collect();
            wait(&fds, deadline.map(|d| d.saturating_duration_since(Instant::now())))?;
        }

        let exposed = output.drain_events()?;
        if !exposed.is_empty() {
            for area in exposed {
                let bands: Vec<Rect> = letterbox.iter().filter_map(|b| b.intersect(&area)).collect();
                output.paint(visible.and_then(|v| v.intersect(&area)), &bands, background)?;
            }
            output.sync()?;
        }

        let blocked_before = stops.covered || stops.battery;
        if let Some(connection) = i3.as_mut() {
            match connection.drain()? {
                i3::Events::None => {}
                i3::Events::Changed => stops.covered = !connection.desktop_visible()?,
                i3::Events::Lost => {
                    // i3 is restarting: keep the last known state until it's back.
                    i3 = None;
                    i3_retry = Some(Instant::now() + I3_RETRY);
                }
            }
        }
        if let (None, Some(retry), Some(path)) = (&i3, i3_retry, &i3_path)
            && retry <= Instant::now()
        {
            match i3::I3::connect(path) {
                Ok(mut connection) => {
                    stops.covered = !connection.desktop_visible()?;
                    i3 = Some(connection);
                    i3_retry = None;
                }
                Err(_) => i3_retry = Some(Instant::now() + I3_RETRY),
            }
        }
        if let Some(p) = &power
            && p.drain()
        {
            stops.battery = p.on_battery();
        }
        if let Some(t) = &thermal {
            let unblocked = blocked_before && !stops.covered && !stops.battery;
            if !stops.covered && !stops.battery && (unblocked || Instant::now() >= next_temperature) {
                let celsius = t.celsius()?;
                stops.hot = t.too_hot(celsius, stops.hot.is_some()).then_some(celsius);
                next_temperature = Instant::now() + TEMPERATURE_INTERVAL;
            }
        }

        if stops.running() != running {
            running = stops.running();
            println!("{}", stops.describe());
            if running {
                let now = Instant::now();
                for animation in scene.layers.iter_mut().filter_map(|l| l.animation.as_mut()) {
                    animation.resume(now, &mut rng, min_hold);
                }
            }
        }
    }
}

/// Block until any descriptor is readable or `timeout` passes (None: no timeout).
fn wait(fds: &[BorrowedFd], timeout: Option<Duration>) -> Result<()> {
    let timespec = timeout.map(|d| Timespec { tv_sec: d.as_secs() as _, tv_nsec: d.subsec_nanos() as _ });
    let mut polled: Vec<PollFd> = fds.iter().map(|fd| PollFd::new(fd, PollFlags::IN)).collect();
    match rustix::event::poll(&mut polled, timespec.as_ref()) {
        Ok(_) | Err(rustix::io::Errno::INTR) => Ok(()),
        Err(e) => Err(e.into()),
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
        palette: "neutral-lift".into(),
        fit: Fit::Cover,
        output: Target::Auto,
        sky: None,
        max_fps: 20.0,
        max_rects: 16,
        max_temp: None,
        ignore_covered: false,
        ignore_battery: false,
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
            "--max-temp" => {
                let raw = value()?;
                args.max_temp = Some(raw.parse().map_err(|_| format!("--max-temp {raw}: expected whole degrees C, e.g. 80"))?);
            }
            "--ignore-covered" => args.ignore_covered = true,
            "--ignore-battery" => args.ignore_battery = true,
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
