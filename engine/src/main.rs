//! molokolive: animated scene packs (molokolive-pack) behind the desktop.
//!
//! Each frame only the canvas rectangles a timeline step changed are recomposited, converted through the palette
//! LUT at native size and uploaded; the X server then scales them onto the output in one grabbed burst (see
//! output.rs). Between steps the process sleeps in poll until the next one is due.
//!
//! Animation runs only while the desktop can be seen (i3), the machine is on mains, and the CPU is below --max-temp.
//! Otherwise every timer stops, the last frame stays on screen, and the process waits for events.
//!
//! Scenes rotate in a shuffle: after --autoplay seconds of animation, or on `molokolive next`/`prev`, with a random
//! sky each time (or each scene's last one, with --persist-sky). The same binary sends those commands.

mod control;
mod i3;
mod output;
mod pack;
mod power;
mod render;

use std::collections::HashMap;
use std::os::fd::BorrowedFd;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use output::{Output, Target};
use pack::Rect;
use power::{Power, Thermal};
use render::{Fit, Geometry};
use rustix::event::{PollFd, PollFlags, Timespec};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// println! that stops quietly when stdout is gone (e.g. `molokolive --scenes --list | head`) instead of panicking.
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

const HELP: &str =
    "molokolive: scenes from \"Milk outside a bag of milk outside a bag of milk\", animated as your wallpaper

Usage:
  molokolive [OPTIONS]      start the wallpaper
  molokolive COMMAND        control the running wallpaper

Commands:
  next                      show the next scene
  prev                      go back to the scene before
  sky-next, sky-prev        change the current scene's sky
  pause, resume             pause or resume the animation
  status                    show the scene, its sky, and whether it's paused

Scenes:
  --scenes A,B,...|all      rotate through these scenes only (default: all)
  --skip-scenes A,B,...     leave these scenes out; in both, * matches any text, e.g. 'mini_cg_*'
  --scenes --list           list the scenes you can use, or the ones --scenes and --skip-scenes pick
  --start-scene NAME        start with this scene (default: a random one)
  --autoplay SECONDS|off    change scene after this many seconds of animation (default: 60)
  --sky N                   start with sky N (`molokolive status` shows sky numbers)
  --persist-sky             give each scene back the sky it had last time
  --no-drift                keep the skies still

Look:
  --palette NAME            colour palette (default: neutral-lift)
  --fit cover|contain       fill the screen and crop the edges, or show the whole scene
                            with borders (default: cover)

Pausing:
  The animation pauses by itself while windows cover the desktop and while on battery.
  --max-temp DEGREES        also pause while the CPU is at least this hot, in °C (default: no limit)
  --ignore-covered          keep animating behind windows
  --ignore-battery          keep animating on battery

Files:
  --packs DIR               scene packs built by molokolive-pack
                            (default: ~/.local/share/molokolive/packs)

  -h, --help                show this help
  --help-all                also show the advanced options

Examples:
  molokolive --autoplay 120 --max-temp 80
  molokolive --scenes cg_floor,cg_firefly --persist-sky
  molokolive --skip-scenes 'mini_cg_*',cg_pills
  molokolive next";

const HELP_ADVANCED: &str = "Advanced:
  --output auto|window|root
                            draw into a desktop window (needed with a compositor such as picom) or
                            onto the root window (without one); auto decides by whether a compositor
                            is running at startup (default: auto)
  --max-fps FPS             speed limit for the fastest animations (default: 20, the game's own speed)
  --max-rects N             most separate screen areas drawn per animation frame; more are merged
                            (default: 16)
  --once                    put the first frame on the root window and exit, without animating
  --stats                   print drawing statistics every 10 seconds";

/// How often the temperature is read while animating, or while stopped for heat.
const TEMPERATURE_INTERVAL: Duration = Duration::from_secs(5);
/// How long to wait before reconnecting after i3 restarts.
const I3_RETRY: Duration = Duration::from_secs(2);
/// Scenes remembered for `prev`.
const HISTORY: usize = 100;
/// Draw limit for frames with a drift step. Their changes are thin and spread over the whole canvas, so merging them
/// into a few strips would damage 40–90% of the screen instead of 9–31% (measured), for ~1 ms of X time once a second.
const DRIFT_MAX_RECTS: usize = 256;

struct Args {
    packs: PathBuf,
    scenes: Option<Vec<String>>,
    skip_scenes: Vec<String>,
    start_scene: Option<String>,
    autoplay: Option<Duration>,
    persist_sky: bool,
    drift: bool,
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
    manual: bool,
}

impl Stops {
    fn running(&self) -> bool {
        !self.covered && !self.battery && self.hot.is_none() && !self.manual
    }

    fn describe(&self) -> String {
        let mut reasons = Vec::new();
        if self.manual {
            reasons.push("by hand".to_string());
        }
        if self.covered {
            reasons.push("desktop covered".to_string());
        }
        if self.battery {
            reasons.push("on battery".to_string());
        }
        if let Some(c) = self.hot {
            reasons.push(format!("too hot ({c} °C)"));
        }
        if reasons.is_empty() {
            "animating".into()
        } else {
            format!("paused: {}", reasons.join(", "))
        }
    }
}

/// xorshift64: hold choices, pools and the shuffle only need to look random.
pub struct Rng(u64);

impl Rng {
    fn seeded() -> Rng {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        Rng(nanos | 1)
    }

    pub fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// Scene order: a shuffled deck, dealt again once every scene has shown, with a history for `prev`.
struct Rotation {
    scenes: Vec<String>,
    deck: Vec<String>,
    history: Vec<String>,
    position: usize,
}

impl Rotation {
    fn new(scenes: Vec<String>, first: Option<String>, rng: &mut Rng) -> Rotation {
        let mut rotation = Rotation {
            scenes,
            deck: Vec::new(),
            history: Vec::new(),
            position: 0,
        };
        let first = match first {
            Some(name) => {
                // The starting scene has shown: deal the rest of the first deck without it.
                rotation.deal_deck(rng);
                rotation.deck.retain(|s| *s != name);
                name
            }
            None => rotation.deal(rng),
        };
        rotation.history.push(first);
        rotation
    }

    fn current(&self) -> &str {
        &self.history[self.position]
    }

    /// Forward through the history, or a new scene from the deck.
    fn next(&mut self, rng: &mut Rng) -> String {
        if self.position + 1 < self.history.len() {
            self.position += 1;
        } else {
            let scene = self.deal(rng);
            self.history.push(scene);
            if self.history.len() > HISTORY {
                self.history.remove(0);
            }
            self.position = self.history.len() - 1;
        }
        self.current().to_string()
    }

    /// Back through the history; None at its start.
    fn prev(&mut self) -> Option<String> {
        self.position = self.position.checked_sub(1)?;
        Some(self.current().to_string())
    }

    fn deal_deck(&mut self, rng: &mut Rng) {
        self.deck = self.scenes.clone();
        for i in (1..self.deck.len()).rev() {
            self.deck.swap(i, rng.below(i + 1));
        }
    }

    fn deal(&mut self, rng: &mut Rng) -> String {
        if self.deck.is_empty() {
            self.deal_deck(rng);
        }
        // Never the scene already showing, when there is another.
        if self.deck.len() > 1 && self.history.get(self.position) == self.deck.last() {
            let last = self.deck.len() - 1;
            self.deck.swap(0, last);
        }
        self.deck.pop().unwrap_or_else(|| self.scenes[0].clone())
    }
}

/// The scene on screen and everything derived from it.
struct Show {
    name: String,
    scene: pack::Scene,
    geometry: Geometry,
    frame: Vec<u8>,
    visible: Option<Rect>,
    letterbox: Vec<Rect>,
    background: [u8; 4],
    /// A full recomposite after a drift step, to compare with `frame`.
    scratch: Vec<u8>,
}

impl Show {
    /// Load a scene, picking each pool's image: `sky` for the sky layer if given, the scene's remembered image with
    /// --persist-sky, else a random one. Records the picks. Doesn't touch the output.
    fn open(
        args: &Args,
        name: &str,
        output: &Output,
        rng: &mut Rng,
        skies: &mut HashMap<(String, String), usize>,
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
                && let Some(&index) = skies.get(&(name.to_string(), layer.to_string()))
                && index < sources.len()
            {
                return Ok(index);
            }
            Ok(rng.below(sources.len()))
        };
        let scene = pack::load(&args.packs.join(name), &args.palette, &mut pick)?;
        for (layer, index) in scene.choices() {
            skies.insert((name.to_string(), layer), index);
        }
        let geometry = Geometry::new(scene.width, scene.height, output.width, output.height, args.fit);
        let canvas = Rect {
            x0: 0,
            y0: 0,
            x1: scene.width,
            y1: scene.height,
        };
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

    fn canvas(&self) -> Rect {
        Rect {
            x0: 0,
            y0: 0,
            x1: self.scene.width,
            y1: self.scene.height,
        }
    }

    /// Give the output a source picture for this scene and upload the whole canvas into it.
    fn upload(&self, output: &mut Output) -> Result<()> {
        output.prepare(self.scene.width, self.scene.height, &self.geometry)?;
        let canvas = self.canvas();
        render::convert(
            &self.frame,
            self.scene.width,
            canvas,
            &self.scene.lut,
            output.image(0, canvas.area() * 4),
        );
        output.upload(canvas, 0)
    }

    fn start(&mut self, rng: &mut Rng, min_hold: Duration, drift: bool) {
        let now = Instant::now();
        for animation in self.scene.layers.iter_mut().filter_map(|l| l.animation.as_mut()) {
            animation.start(now, rng, min_hold);
        }
        if drift {
            self.scene.start_drift(now);
        }
    }

    /// Canvas areas where the layers now differ from the frame on screen, as runs of 16 px tiles. A drift step
    /// moves a whole layer, but only pixels where it shows and differs from its neighbour change.
    fn changed_tiles(&mut self, dirty: &mut Vec<Rect>) {
        const TILE: usize = 16;
        let (width, height) = (self.scene.width, self.scene.height);
        let canvas = self.canvas();
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

    fn describe(&self) -> String {
        let picks = self.scene.picks();
        if picks.is_empty() {
            self.name.clone()
        } else {
            format!("{} ({picks})", self.name)
        }
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let fail = |message: String| {
        eprintln!("molokolive: {message}");
        ExitCode::FAILURE
    };
    if let Some(command) = argv.first().filter(|a| !a.starts_with('-')) {
        if !control::COMMANDS.contains(&command.as_str()) {
            return fail(format!(
                "unknown command '{command}' (commands: {})",
                control::COMMANDS.join(", ")
            ));
        }
        if argv.len() > 1 {
            return fail(format!("'{command}' doesn't take options"));
        }
        return match control::send(command) {
            Ok(reply) if reply.starts_with("error") => fail(reply.trim_start_matches("error: ").to_string()),
            Ok(reply) => {
                say!("{reply}");
                ExitCode::SUCCESS
            }
            Err(e) => fail(e.to_string()),
        };
    }
    match parse_args(argv).and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(e.to_string()),
    }
}

fn run(args: Args) -> Result<()> {
    let started = Instant::now();
    let mut rng = Rng::seeded();
    let scenes = select_scenes(&args)?;
    let thermal = args.max_temp.map(Thermal::open).transpose()?;
    let mut rotation = Rotation::new(scenes, args.start_scene.clone(), &mut rng);
    let mut skies = HashMap::new();

    let mut output = Output::connect(args.output)?;
    let mut show = Show::open(&args, rotation.current(), &output, &mut rng, &mut skies, args.sky)?;
    show.upload(&mut output)?;
    output.publish(show.visible, &show.letterbox, show.background)?;
    let animated = show.scene.layers.iter().filter(|l| l.animation.is_some()).count();
    say!(
        "{}: canvas {}x{} -> {}x{} at x{:.3} on the {}, {} layers ({animated} animated), ready in {:.1} ms",
        show.describe(),
        show.scene.width,
        show.scene.height,
        show.geometry.width,
        show.geometry.height,
        show.geometry.scale,
        output.kind(),
        show.scene.layers.len(),
        ms(started.elapsed())
    );
    if args.once {
        return Ok(());
    }
    // Publishing has ended any engine that was running, so the socket is ours to take.
    let server = control::Server::bind()?;

    // What may stop animation, and its state now.
    let i3_path = std::env::var("I3SOCK")
        .ok()
        .filter(|p| !p.is_empty())
        .or_else(|| output.i3_socket_path());
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
        manual: false,
    };
    if let Some(t) = &thermal {
        let celsius = t.celsius()?;
        stops.hot = t.too_hot(celsius, false).then_some(celsius);
    }
    let mut next_temperature = Instant::now() + TEMPERATURE_INTERVAL;
    let mut running = stops.running();
    say!("{}", stops.describe());

    let min_hold = Duration::from_secs_f64(1.0 / args.max_fps);
    show.start(&mut rng, min_hold, args.drift);
    // Animation time on the current scene, which is what --autoplay counts.
    let (mut played, mut last_tick) = (Duration::ZERO, Instant::now());
    let (mut frames, mut rects, mut pixels, mut report) = (0u64, 0u64, 0u64, Instant::now());
    let [mut t_composite, mut t_convert, mut t_x] = [Duration::ZERO; 3];
    let (mut dirty, mut shown) = (Vec::new(), Vec::new());
    let mut drifted = false;
    loop {
        let now = Instant::now();
        if running {
            played += now - last_tick;
        }
        last_tick = now;
        if let Some(interval) = args.autoplay
            && running
            && played >= interval
        {
            let name = rotation.next(&mut rng);
            if let Err(e) = change_scene(&args, &name, &mut output, &mut rng, &mut skies, &mut show, min_hold) {
                eprintln!("molokolive: {e}");
            }
            (played, last_tick) = (Duration::ZERO, Instant::now());
            dirty.clear();
        }

        if running {
            let now = Instant::now();
            let width = show.scene.width;
            for layer in &mut show.scene.layers {
                let Some(animation) = layer.animation.as_mut() else {
                    continue;
                };
                while animation.due.is_some_and(|due| due <= now) {
                    animation.advance(&mut layer.pixels, width, now, &mut rng, min_hold, &mut dirty);
                }
            }
            if show.scene.advance_drift(now) {
                show.changed_tiles(&mut dirty);
                drifted = true;
            }
        }
        if !dirty.is_empty() {
            // Upload every changed rectangle into the source first (nothing visible yet), then scale them all onto
            // the screen at once.
            let mut offset = 0;
            let max_rects = if drifted {
                args.max_rects.max(DRIFT_MAX_RECTS)
            } else {
                args.max_rects
            };
            for rect in merge(&dirty, max_rects) {
                let t0 = Instant::now();
                show.scene.composite(&mut show.frame, rect);
                let len = rect.area() * 4;
                if offset + len > output.capacity() {
                    output.sync()?; // the server has read what we uploaded so far
                    offset = 0;
                }
                let t1 = Instant::now();
                render::convert(
                    &show.frame,
                    show.scene.width,
                    rect,
                    &show.scene.lut,
                    output.image(offset, len),
                );
                let t2 = Instant::now();
                output.upload(rect, offset)?;
                shown.extend(show.geometry.to_output(&rect));
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
            drifted = false;
            frames += 1;
            if args.stats && report.elapsed() >= Duration::from_secs(10) {
                let secs = report.elapsed().as_secs_f64();
                let per_frame = |d: Duration| ms(d) / frames as f64;
                say!(
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

        // Sleep until something is due: the next animation step or scene change while running, a temperature reading
        // while only heat could be stopping us, an i3 reconnect attempt. With none of those, block on events alone.
        let checking_heat = thermal.is_some() && !stops.covered && !stops.battery;
        let deadline = [
            if running {
                show.scene.layers.iter().filter_map(|l| l.animation.as_ref()?.due).min()
            } else {
                None
            },
            if running { show.scene.drift_due() } else { None },
            args.autoplay
                .filter(|_| running)
                .map(|interval| Instant::now() + interval.saturating_sub(played)),
            checking_heat.then_some(next_temperature),
            i3_retry,
        ]
        .into_iter()
        .flatten()
        .min();
        {
            let fds: Vec<BorrowedFd> = [
                Some(output.fd()),
                Some(server.fd()),
                i3.as_ref().map(|c| c.fd()),
                power.as_ref().and_then(Power::fd),
            ]
            .into_iter()
            .flatten()
            .collect();
            wait(&fds, deadline.map(|d| d.saturating_duration_since(Instant::now())))?;
        }

        let exposed = output.drain_events()?;
        if !exposed.is_empty() {
            for area in exposed {
                let bands: Vec<Rect> = show.letterbox.iter().filter_map(|b| b.intersect(&area)).collect();
                output.paint(show.visible.and_then(|v| v.intersect(&area)), &bands, show.background)?;
            }
            output.sync()?;
        }

        for request in server.accept() {
            let reply = match request.command.as_str() {
                "next" | "prev" => {
                    let target = if request.command == "next" {
                        Some(rotation.next(&mut rng))
                    } else {
                        rotation.prev()
                    };
                    match target {
                        None => format!("{} (no earlier scene)", show.describe()),
                        Some(name) => {
                            match change_scene(&args, &name, &mut output, &mut rng, &mut skies, &mut show, min_hold) {
                                Ok(()) => {
                                    (played, last_tick) = (Duration::ZERO, Instant::now());
                                    dirty.clear();
                                    show.describe()
                                }
                                Err(e) => format!("error: {e}"),
                            }
                        }
                    }
                }
                "sky-next" | "sky-prev" => {
                    let step = if request.command == "sky-next" { 1 } else { -1 };
                    match show.scene.step_choice("sky", step) {
                        Ok(Some(index)) => {
                            skies.insert((show.name.clone(), "sky".to_string()), index);
                            dirty.push(show.canvas());
                            show.describe()
                        }
                        Ok(None) => format!("{} has no sky", show.name),
                        Err(e) => format!("error: {e}"),
                    }
                }
                "pause" | "resume" => {
                    stops.manual = request.command == "pause";
                    stops.describe()
                }
                "status" => {
                    let autoplay = match args.autoplay {
                        Some(interval) => format!(
                            "; next scene after {} s more animation",
                            interval.saturating_sub(played).as_secs()
                        ),
                        None => String::new(),
                    };
                    format!("{}: {}{autoplay}", show.describe(), stops.describe())
                }
                other => format!(
                    "error: unknown command {other:?} (commands: {})",
                    control::COMMANDS.join(", ")
                ),
            };
            request.reply(&reply);
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
            say!("{}", stops.describe());
            if running {
                let now = Instant::now();
                last_tick = now;
                for animation in show.scene.layers.iter_mut().filter_map(|l| l.animation.as_mut()) {
                    animation.resume(now, &mut rng, min_hold);
                }
                show.scene.resume_drift(now);
            }
        }
    }
}

/// Replace the scene on screen with `name` (a hard cut). A pack that fails to load leaves the current scene up.
fn change_scene(
    args: &Args,
    name: &str,
    output: &mut Output,
    rng: &mut Rng,
    skies: &mut HashMap<(String, String), usize>,
    show: &mut Show,
    min_hold: Duration,
) -> Result<()> {
    let mut next = Show::open(args, name, output, rng, skies, None)?;
    next.upload(output)?;
    output.paint(next.visible, &next.letterbox, next.background)?;
    output.sync()?;
    next.start(rng, min_hold, args.drift);
    *show = next;
    say!("{}", show.describe());
    Ok(())
}

/// Every directory in `packs` with a manifest, sorted.
fn available_scenes(packs: &Path) -> Result<Vec<String>> {
    let entries = std::fs::read_dir(packs).map_err(|_| {
        format!(
            "no scene packs in {} (build them with molokolive-pack, or pass --packs)",
            packs.display()
        )
    })?;
    let mut scenes: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().join("manifest.json").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    scenes.sort();
    if scenes.is_empty() {
        return Err(format!(
            "no scene packs in {} (build them with molokolive-pack, or pass --packs)",
            packs.display()
        )
        .into());
    }
    Ok(scenes)
}

/// Block until any descriptor is readable or `timeout` passes (None: no timeout).
fn wait(fds: &[BorrowedFd], timeout: Option<Duration>) -> Result<()> {
    let timespec = timeout.map(|d| Timespec {
        tv_sec: d.as_secs() as _,
        tv_nsec: d.subsec_nanos() as _,
    });
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

/// The scenes to rotate through: --scenes (every scene by default) minus --skip-scenes, both with `*` wildcards.
fn select_scenes(args: &Args) -> Result<Vec<String>> {
    let available = available_scenes(&args.packs)?;
    let matching = |patterns: &[String], option: &str| -> Result<Vec<String>> {
        let mut found = Vec::new();
        for pattern in patterns {
            let matched: Vec<String> = available.iter().filter(|s| wildcard(pattern, s)).cloned().collect();
            if matched.is_empty() {
                return Err(
                    format!("{option}: no scene matches '{pattern}' (molokolive --scenes --list shows them)").into(),
                );
            }
            found.extend(matched);
        }
        Ok(found)
    };
    let mut scenes = match &args.scenes {
        Some(patterns) => matching(patterns, "--scenes")?,
        None => available.clone(),
    };
    let skipped = matching(&args.skip_scenes, "--skip-scenes")?;
    scenes.retain(|s| !skipped.contains(s));
    scenes.sort();
    scenes.dedup();
    if scenes.is_empty() {
        return Err("--skip-scenes leaves no scenes to show".into());
    }
    if let Some(start) = &args.start_scene {
        if !available.contains(start) {
            return Err(format!("no scene '{start}' (molokolive --scenes --list shows them)").into());
        }
        if !scenes.contains(start) {
            return Err(format!("--start-scene {start} isn't in the rotation (see --scenes and --skip-scenes)").into());
        }
    }
    Ok(scenes)
}

/// Whether `name` matches `pattern`, where each `*` stands for any run of characters.
fn wildcard(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let [first, middle @ .., last] = parts.as_slice() else {
        return pattern == name;
    };
    if name.len() < first.len() + last.len() || !name.starts_with(first) || !name.ends_with(last) {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

/// Print the scenes with what each has, for `--scenes --list`: every scene, or the ones --scenes and
/// --skip-scenes pick.
fn print_scenes(args: &Args) -> Result<()> {
    let everything = available_scenes(&args.packs)?;
    let filtered = args.scenes.is_some() || !args.skip_scenes.is_empty();
    let scenes = if filtered {
        select_scenes(args)?
    } else {
        everything.clone()
    };
    let packs = args.packs.as_path();
    let home = std::env::var("HOME").unwrap_or_default();
    let shown = packs.display().to_string();
    let shown = match shown.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => shown,
    };
    if filtered {
        say!(
            "{} of the {} scenes in {shown}, as --scenes and --skip-scenes pick them:\n",
            scenes.len(),
            everything.len()
        );
    } else {
        say!("Scenes in {shown}:\n");
    }
    let width = scenes.iter().map(String::len).max().unwrap_or(0);
    for name in &scenes {
        let text = std::fs::read_to_string(packs.join(name).join("manifest.json"))?;
        let manifest: serde_json::Value = serde_json::from_str(&text)?;
        let layers = manifest["layers"].as_array().cloned().unwrap_or_default();
        let pool = |layer: &str| {
            layers
                .iter()
                .any(|l| l["name"] == layer && l["choices"].as_array().is_some_and(|c| c.len() > 1))
        };
        let mut features = Vec::new();
        if pool("sky") {
            features.push("sky");
        }
        if pool("reflection") {
            features.push("reflection");
        }
        features.push(if layers.iter().any(|l| l.get("steps").is_some()) {
            "animated"
        } else {
            "still"
        });
        say!("  {name:width$}   {}", features.join(", "));
    }
    say!(
        "\nUse the names with --scenes, e.g. molokolive --scenes {},{}, or with --start-scene.",
        everything[0],
        everything.get(1).unwrap_or(&everything[0])
    );
    Ok(())
}

/// Where molokolive-pack writes packs: $XDG_DATA_HOME/molokolive/packs, else ~/.local/share/molokolive/packs.
fn default_packs() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    data.join("molokolive").join("packs")
}

fn parse_args(argv: Vec<String>) -> Result<Args> {
    let mut args = Args {
        packs: default_packs(),
        scenes: None,
        skip_scenes: Vec::new(),
        start_scene: None,
        autoplay: Some(Duration::from_secs(60)),
        persist_sky: false,
        drift: true,
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
    let mut list_scenes = false;
    // `--option=value` is the same as `--option value`.
    let argv = argv.into_iter().flat_map(|arg| match arg.split_once('=') {
        Some((option, value)) if option.starts_with("--") => vec![option.to_string(), value.to_string()],
        _ => vec![arg],
    });
    let mut it = argv.peekable();
    while let Some(arg) = it.next() {
        if matches!(arg.as_str(), "--scenes" | "--skip-scenes" | "--start-scene") {
            match it.peek().map(String::as_str) {
                // Explicitly asked: list the scenes.
                Some("--list" | "--help" | "-h") => {
                    it.next();
                    list_scenes = true;
                    continue;
                }
                // Nothing given: keep the default (every scene, none skipped, a random start).
                None => continue,
                Some(next) if next.starts_with('-') => continue,
                _ => {}
            }
        }
        let mut value = || {
            it.next()
                .ok_or_else(|| format!("{arg} needs a value (see molokolive --help)"))
        };
        match arg.as_str() {
            "--packs" => args.packs = value()?.into(),
            "--scenes" => {
                let raw = value()?;
                if raw == "all" {
                    args.scenes = None;
                    continue;
                }
                let list: Vec<String> = raw
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
                if list.is_empty() {
                    return Err(
                        "--scenes needs at least one scene name (molokolive --scenes --list shows them)".into(),
                    );
                }
                args.scenes = Some(list);
            }
            "--skip-scenes" => {
                args.skip_scenes = value()?
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
            }
            "--start-scene" => args.start_scene = Some(value()?),
            "--autoplay" => {
                let raw = value()?;
                args.autoplay = match raw.as_str() {
                    "off" => None,
                    _ => match raw.parse::<f64>() {
                        Ok(secs) if secs > 0.0 => Some(Duration::from_secs_f64(secs)),
                        _ => return Err(format!("--autoplay {raw}: expected a number of seconds, or off").into()),
                    },
                }
            }
            "--persist-sky" => args.persist_sky = true,
            "--no-drift" => args.drift = false,
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
                    other => return Err(format!("--output {other}: expected auto, window or root").into()),
                }
            }
            "--sky" => {
                let raw = value()?;
                args.sky = Some(
                    raw.parse()
                        .map_err(|_| format!("--sky {raw}: expected a sky number, e.g. 12"))?,
                );
            }
            "--max-fps" => {
                let raw = value()?;
                args.max_fps = raw
                    .parse::<f64>()
                    .map_err(|_| format!("--max-fps {raw}: expected a number"))?
                    .max(0.1);
            }
            "--max-rects" => {
                let raw = value()?;
                args.max_rects = raw
                    .parse::<usize>()
                    .map_err(|_| format!("--max-rects {raw}: expected a whole number"))?
                    .max(1);
            }
            "--max-temp" => {
                let raw = value()?;
                args.max_temp = Some(
                    raw.parse()
                        .map_err(|_| format!("--max-temp {raw}: expected whole degrees C, e.g. 80"))?,
                );
            }
            "--ignore-covered" => args.ignore_covered = true,
            "--ignore-battery" => args.ignore_battery = true,
            "--once" => args.once = true,
            "--stats" => args.stats = true,
            "-h" | "--help" => {
                say!("{HELP}");
                std::process::exit(0);
            }
            "--help-all" => {
                say!("{HELP}\n\n{HELP_ADVANCED}");
                std::process::exit(0);
            }
            "--scene" => return Err("--scene is now --start-scene".into()),
            "--persist-skybox" => return Err("--persist-skybox is now --persist-sky".into()),
            _ => return Err(format!("unknown option '{arg}' (see molokolive --help)").into()),
        }
    }
    if list_scenes {
        print_scenes(&args)?;
        std::process::exit(0);
    }
    if args.once {
        // A window disappears with the process; only the root background outlives it.
        if args.output == Target::Window {
            return Err("--once draws onto the root window, so it can't be combined with --output window".into());
        }
        args.output = Target::Root;
    }
    Ok(args)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
