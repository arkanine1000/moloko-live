//! The engine: one loop that animates, draws, sleeps until something is due, and reacts to events and commands.

use std::os::fd::BorrowedFd;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec};

use crate::cli::Args;
use crate::control::{self, Command};
use crate::frame::{self, DRIFT_MAX_RECTS, Stats, ms};
use crate::geometry::Rect;
use crate::output::Output;
use crate::pause::Pause;
use crate::rotation::{Rng, Rotation};
use crate::show::{Picks, Show};
use crate::{Result, render, say, scenes};

pub struct Engine {
    args: Args,
    rng: Rng,
    rotation: Rotation,
    picks: Picks,
    output: Output,
    show: Show,
    server: control::Server,
    pause: Pause,
    running: bool,
    /// The shortest hold, from --max-fps.
    min_hold: Duration,
    /// Animation time on the current scene, which is what --autoplay counts.
    played: Duration,
    last_tick: Instant,
    /// Canvas rectangles changed since the last frame.
    dirty: Vec<Rect>,
    /// Whether one of them comes from a drift step.
    drifted: bool,
    stats: Option<Stats>,
}

/// Start the wallpaper and run it until an error ends it.
pub fn run(args: Args) -> Result<()> {
    let started = Instant::now();
    let mut rng = Rng::seeded();
    let scenes = scenes::select(&args)?;
    let rotation = Rotation::new(scenes, args.start_scene.clone(), args.shuffle, &mut rng);
    let mut picks = Picks::new();
    let mut output = Output::connect(args.output)?;
    let show = Show::open(&args, rotation.current(), &output, &mut rng, &mut picks, args.sky)?;
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
    let pause = Pause::open(&args, &output)?;
    let mut engine = Engine {
        running: pause.running(),
        min_hold: Duration::from_secs_f64(1.0 / args.max_fps),
        played: Duration::ZERO,
        last_tick: Instant::now(),
        dirty: Vec::new(),
        drifted: false,
        stats: args.stats.then(Stats::new),
        rotation,
        args,
        rng,
        picks,
        output,
        show,
        server,
        pause,
    };
    engine.show.start(&mut engine.rng, engine.min_hold, engine.args.drift);
    loop {
        engine.tick()?;
        engine.animate();
        engine.draw()?;
        engine.wait()?;
        engine.repaint_exposed()?;
        engine.serve()?;
        engine.pause.poll()?;
        engine.update_running();
    }
}

impl Engine {
    /// Count animation time and change scene when --autoplay says so.
    fn tick(&mut self) -> Result<()> {
        let now = Instant::now();
        if self.running {
            self.played += now - self.last_tick;
        }
        self.last_tick = now;
        if let Some(interval) = self.args.autoplay.0
            && self.running
            && self.played >= interval
        {
            let name = self.rotation.next(&mut self.rng);
            if let Err(e) = self.change_scene(&name) {
                eprintln!("molokolive: {e}");
            }
        }
        Ok(())
    }

    /// Take every animation and drift step that is due.
    fn animate(&mut self) {
        if !self.running {
            return;
        }
        let now = Instant::now();
        let width = self.show.scene.width;
        for layer in &mut self.show.scene.layers {
            let Some(animation) = layer.animation.as_mut() else {
                continue;
            };
            while animation.due.is_some_and(|due| due <= now) {
                animation.advance(
                    &mut layer.pixels,
                    width,
                    now,
                    &mut self.rng,
                    self.min_hold,
                    &mut self.dirty,
                );
            }
        }
        if self.show.scene.advance_drift(now) {
            self.show.changed_tiles(&mut self.dirty);
            self.drifted = true;
        }
    }

    /// Put the changed rectangles on screen as one frame.
    fn draw(&mut self) -> Result<()> {
        if self.dirty.is_empty() {
            return Ok(());
        }
        // Upload every changed rectangle into the source first (nothing visible yet), then scale them all onto
        // the screen at once.
        let max_rects = if self.drifted {
            self.args.max_rects.max(DRIFT_MAX_RECTS)
        } else {
            self.args.max_rects
        };
        let mut offset = 0;
        let mut shown = Vec::new();
        for rect in frame::merge(&self.dirty, max_rects) {
            let t0 = Instant::now();
            self.show.scene.composite(&mut self.show.frame, rect);
            let len = rect.area() * 4;
            if offset + len > self.output.capacity() {
                self.output.sync()?; // the server has read what we uploaded so far
                offset = 0;
            }
            let t1 = Instant::now();
            let scene = &self.show.scene;
            render::convert(
                &self.show.frame,
                scene.width,
                rect,
                &scene.lut,
                self.output.image(offset, len),
            );
            let t2 = Instant::now();
            self.output.upload(rect, offset)?;
            shown.extend(self.show.geometry.to_output(&rect));
            if let Some(stats) = &mut self.stats {
                stats.rect(rect.area(), t1 - t0, t2 - t1, t2.elapsed());
            }
            offset += len;
        }
        let t3 = Instant::now();
        self.output.show_frame(&shown)?;
        self.output.sync()?;
        if let Some(stats) = &mut self.stats {
            stats.frame(t3.elapsed());
        }
        self.dirty.clear();
        self.drifted = false;
        Ok(())
    }

    /// Sleep until something is due: the next animation step or scene change while running, a pause check, or
    /// an event on any connection.
    fn wait(&self) -> Result<()> {
        let scene = &self.show.scene;
        let deadline = [
            if self.running { scene.animation_due() } else { None },
            if self.running { scene.drift_due() } else { None },
            self.args
                .autoplay
                .0
                .filter(|_| self.running)
                .map(|interval| Instant::now() + interval.saturating_sub(self.played)),
            self.pause.deadline(),
        ]
        .into_iter()
        .flatten()
        .min();
        let mut fds = vec![self.output.fd(), self.server.fd()];
        fds.extend(self.pause.fds());
        poll(&fds, deadline.map(|d| d.saturating_duration_since(Instant::now())))
    }

    /// Repaint the screen areas X says were exposed.
    fn repaint_exposed(&mut self) -> Result<()> {
        let exposed = self.output.drain_events()?;
        if exposed.is_empty() {
            return Ok(());
        }
        for area in exposed {
            let bands: Vec<Rect> = self.show.letterbox.iter().filter_map(|b| b.intersect(&area)).collect();
            self.output.paint(
                self.show.visible.and_then(|v| v.intersect(&area)),
                &bands,
                self.show.background,
            )?;
        }
        self.output.sync()
    }

    /// Answer every waiting control client.
    fn serve(&mut self) -> Result<()> {
        for request in self.server.accept() {
            let reply = match Command::parse(&request.command) {
                Some(command) => self.handle(command),
                None => format!(
                    "error: unknown command {:?} (commands: {})",
                    request.command,
                    Command::list()
                ),
            };
            request.reply(&reply);
        }
        Ok(())
    }

    /// Carry out a command; the reply is one line, starting with "error" when it failed.
    fn handle(&mut self, command: Command) -> String {
        match command {
            Command::Next | Command::Prev => {
                let target = if command == Command::Next {
                    Some(self.rotation.next(&mut self.rng))
                } else {
                    self.rotation.prev()
                };
                match target {
                    None => format!("{} (no earlier scene)", self.show.describe()),
                    Some(name) => match self.change_scene(&name) {
                        Ok(()) => self.show.describe(),
                        Err(e) => format!("error: {e}"),
                    },
                }
            }
            Command::SkyNext | Command::SkyPrev => {
                let step = if command == Command::SkyNext { 1 } else { -1 };
                match self.show.scene.step_choice("sky", step) {
                    Ok(Some(index)) => {
                        self.picks.insert((self.show.name.clone(), "sky".to_string()), index);
                        self.dirty.push(self.show.scene.canvas());
                        self.show.describe()
                    }
                    Ok(None) => format!("{} has no sky", self.show.name),
                    Err(e) => format!("error: {e}"),
                }
            }
            Command::Pause | Command::Resume => {
                self.pause.stops.manual = command == Command::Pause;
                self.pause.stops.describe()
            }
            Command::Status => {
                let autoplay = match self.args.autoplay.0 {
                    Some(interval) => {
                        format!(
                            "; next scene after {} s more animation",
                            interval.saturating_sub(self.played).as_secs()
                        )
                    }
                    None => String::new(),
                };
                format!("{}: {}{autoplay}", self.show.describe(), self.pause.stops.describe())
            }
        }
    }

    /// Replace the scene on screen with `name` (a hard cut) and restart the autoplay clock. A pack that fails to
    /// load leaves the current scene up.
    fn change_scene(&mut self, name: &str) -> Result<()> {
        let mut next = Show::open(&self.args, name, &self.output, &mut self.rng, &mut self.picks, None)?;
        next.upload(&mut self.output)?;
        self.output.paint(next.visible, &next.letterbox, next.background)?;
        self.output.sync()?;
        next.start(&mut self.rng, self.min_hold, self.args.drift);
        self.show = next;
        say!("{}", self.show.describe());
        (self.played, self.last_tick) = (Duration::ZERO, Instant::now());
        self.dirty.clear();
        Ok(())
    }

    /// Start or stop animating as the stops change.
    fn update_running(&mut self) {
        if self.pause.running() == self.running {
            return;
        }
        self.running = self.pause.running();
        say!("{}", self.pause.stops.describe());
        if self.running {
            self.last_tick = Instant::now();
            self.show.resume(&mut self.rng, self.min_hold);
        }
    }
}

/// Block until any descriptor is readable or `timeout` passes (None: no timeout).
fn poll(fds: &[BorrowedFd], timeout: Option<Duration>) -> Result<()> {
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
