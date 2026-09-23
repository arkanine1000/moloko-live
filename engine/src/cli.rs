//! The command line: options for the engine, and the help text.

use std::path::PathBuf;
use std::time::Duration;

use clap::error::ErrorKind;
use clap::{ArgAction, Parser};

use crate::geometry::Fit;
use crate::output::Target;
use crate::{Result, say, scenes};

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
  --shuffle                 play the scenes in random order instead of the order the list shows
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

/// The engine's options. `--option=value` works everywhere.
#[derive(Parser, Debug)]
#[command(name = "molokolive", override_help = HELP, disable_version_flag = true)]
pub struct Args {
    #[arg(long, value_name = "DIR", default_value_os_t = default_packs())]
    pub packs: PathBuf,
    /// Empty means every scene.
    #[arg(long, value_name = "A,B,...", num_args = 0..=1, default_value = "all", default_missing_value = "all", value_parser = scene_list)]
    pub scenes: Names,
    #[arg(long, value_name = "A,B,...", num_args = 0..=1, default_value = "", default_missing_value = "", value_parser = name_list)]
    pub skip_scenes: Names,
    #[arg(long, value_name = "NAME", num_args = 0..=1, default_missing_value = "")]
    pub start_scene: Option<String>,
    #[arg(long)]
    pub shuffle: bool,
    #[arg(long, value_name = "SECONDS|off", default_value = "60", value_parser = autoplay)]
    pub autoplay: Autoplay,
    #[arg(long, value_name = "N")]
    pub sky: Option<u32>,
    #[arg(long)]
    pub persist_sky: bool,
    #[arg(long = "no-drift", action = ArgAction::SetFalse)]
    pub drift: bool,
    #[arg(long, value_name = "NAME", default_value = "neutral-lift")]
    pub palette: String,
    #[arg(long, value_name = "cover|contain", default_value = "cover", value_parser = fit)]
    pub fit: Fit,
    #[arg(long, value_name = "DEGREES")]
    pub max_temp: Option<i32>,
    #[arg(long)]
    pub ignore_covered: bool,
    #[arg(long)]
    pub ignore_battery: bool,
    #[arg(long, value_name = "auto|window|root", default_value = "auto", value_parser = target)]
    pub output: Target,
    #[arg(long, value_name = "FPS", default_value = "20", value_parser = fps)]
    pub max_fps: f64,
    #[arg(long, value_name = "N", default_value = "16", value_parser = rects)]
    pub max_rects: usize,
    #[arg(long)]
    pub once: bool,
    #[arg(long)]
    pub stats: bool,
    /// With --scenes, --skip-scenes or --start-scene: list the scenes instead of starting.
    #[arg(long)]
    list: bool,
    #[arg(long)]
    help_all: bool,
}

/// Seconds of animation before the next scene; None means stay on one scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Autoplay(pub Option<Duration>);

/// A comma-separated list of scene names or patterns, as one option value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Names(pub Vec<String>);

impl std::ops::Deref for Names {
    type Target = [String];

    fn deref(&self) -> &[String] {
        &self.0
    }
}

/// Where molokolive-pack writes packs: $XDG_DATA_HOME/molokolive/packs, else ~/.local/share/molokolive/packs.
pub fn default_packs() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    data.join("molokolive").join("packs")
}

/// Parse the engine's options, or print help or the scene list and exit.
pub fn parse(argv: Vec<String>) -> Result<Args> {
    let argv = std::iter::once("molokolive".to_string()).chain(list_alias(argv));
    let mut args = match Args::try_parse_from(argv) {
        Ok(args) => args,
        Err(e) if e.kind() == ErrorKind::DisplayHelp => {
            say!("{HELP}");
            std::process::exit(0);
        }
        Err(e) => return Err(one_line(&e).into()),
    };
    if args.help_all {
        say!("{HELP}\n\n{HELP_ADVANCED}");
        std::process::exit(0);
    }
    if args.start_scene.as_deref() == Some("") {
        args.start_scene = None;
    }
    if args.list {
        scenes::print(&args)?;
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

/// `--scenes --help` (and -h) means the same as `--scenes --list`.
fn list_alias(argv: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(argv.len());
    for arg in argv {
        let after_scene_option = out
            .last()
            .is_some_and(|last: &String| matches!(last.as_str(), "--scenes" | "--skip-scenes" | "--start-scene"));
        if after_scene_option && matches!(arg.as_str(), "--help" | "-h") {
            out.push("--list".to_string());
        } else {
            out.push(arg);
        }
    }
    out
}

/// clap's first error line, without its prefix, pointing at the help.
fn one_line(e: &clap::Error) -> String {
    let text = e.to_string();
    let line = text
        .lines()
        .next()
        .unwrap_or_default()
        .trim_start_matches("error: ")
        .trim();
    format!("{line} (see molokolive --help)")
}

fn names(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

fn scene_list(raw: &str) -> std::result::Result<Names, String> {
    if raw == "all" {
        return Ok(Names(Vec::new()));
    }
    let list = names(raw);
    if list.is_empty() {
        return Err("needs at least one scene name (molokolive --scenes --list shows them)".into());
    }
    Ok(Names(list))
}

fn name_list(raw: &str) -> std::result::Result<Names, String> {
    Ok(Names(names(raw)))
}

fn autoplay(raw: &str) -> std::result::Result<Autoplay, String> {
    if raw == "off" {
        return Ok(Autoplay(None));
    }
    match raw.parse::<f64>() {
        Ok(secs) if secs > 0.0 => Ok(Autoplay(Some(Duration::from_secs_f64(secs)))),
        _ => Err("expected a number of seconds, or off".into()),
    }
}

fn fit(raw: &str) -> std::result::Result<Fit, String> {
    match raw {
        "cover" => Ok(Fit::Cover),
        "contain" => Ok(Fit::Contain),
        _ => Err("expected cover or contain".into()),
    }
}

fn target(raw: &str) -> std::result::Result<Target, String> {
    match raw {
        "auto" => Ok(Target::Auto),
        "window" => Ok(Target::Window),
        "root" => Ok(Target::Root),
        _ => Err("expected auto, window or root".into()),
    }
}

fn fps(raw: &str) -> std::result::Result<f64, String> {
    raw.parse::<f64>()
        .map(|v| v.max(0.1))
        .map_err(|_| "expected a number".into())
}

fn rects(raw: &str) -> std::result::Result<usize, String> {
    raw.parse::<usize>()
        .map(|v| v.max(1))
        .map_err(|_| "expected a whole number".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> std::result::Result<Args, String> {
        let argv = std::iter::once("molokolive")
            .chain(args.iter().copied())
            .map(String::from);
        Args::try_parse_from(list_alias(argv.collect())).map_err(|e| one_line(&e))
    }

    #[test]
    fn defaults() {
        let args = parse(&[]).unwrap();
        assert!(args.scenes.is_empty() && args.skip_scenes.is_empty() && args.start_scene.is_none());
        assert!(!args.shuffle && args.drift && !args.persist_sky && !args.once);
        assert_eq!(args.autoplay, Autoplay(Some(Duration::from_secs(60))));
        assert_eq!(
            (args.fit, args.output, args.max_fps, args.max_rects),
            (Fit::Cover, Target::Auto, 20.0, 16)
        );
        assert_eq!(args.palette, "neutral-lift");
    }

    #[test]
    fn values_with_equals_and_lists() {
        let args = parse(&[
            "--scenes=cg_floor, mini_cg_*",
            "--skip-scenes",
            "cg_pills",
            "--autoplay",
            "off",
        ])
        .unwrap();
        assert_eq!(*args.scenes, ["cg_floor", "mini_cg_*"]);
        assert_eq!(*args.skip_scenes, ["cg_pills"]);
        assert_eq!(args.autoplay, Autoplay(None));
    }

    #[test]
    fn bare_scene_options_keep_the_defaults_and_list_is_a_flag() {
        let args = parse(&["--scenes", "--list"]).unwrap();
        assert!(args.scenes.is_empty() && args.list);
        let args = parse(&["--scenes", "all", "--start-scene", "--no-drift"]).unwrap();
        assert!(args.scenes.is_empty() && args.start_scene.as_deref() == Some("") && !args.drift);
        assert!(parse(&["--start-scene", "--help"]).unwrap().list);
    }

    #[test]
    fn shuffle_and_clamps() {
        assert!(parse(&["--shuffle"]).unwrap().shuffle);
        assert_eq!(parse(&["--max-fps", "0"]).unwrap().max_fps, 0.1);
        assert_eq!(parse(&["--max-rects", "0"]).unwrap().max_rects, 1);
    }

    #[test]
    fn errors_are_one_line() {
        let err = parse(&["--autoplay", "soon"]).unwrap_err();
        assert!(
            err.starts_with("invalid value 'soon' for '--autoplay <SECONDS|off>': expected a number"),
            "{err}"
        );
        assert!(err.ends_with("(see molokolive --help)"));
        assert!(
            parse(&["--scenes", ""])
                .unwrap_err()
                .contains("at least one scene name")
        );
        assert!(parse(&["--frobnicate"]).unwrap_err().contains("--frobnicate"));
    }
}
