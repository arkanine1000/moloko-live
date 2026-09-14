//! molokolive, a first spike: composite one static scene pack (tools/pack.py) and make it the X root
//! background the way feh does, so picom and other root-pixmap readers pick it up.

use std::ffi::OsStr;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, CloseDown, ConnectionExt as _, CreateGCAux, ImageFormat, ImageOrder,
    PropMode,
};
use x11rb::wrapper::ConnectionExt as _;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const USAGE: &str = "usage: molokolive [--pack DIR] [--palette NAME] [--sky N] [--hold]

  --pack DIR       scene pack from tools/pack.py (default rip/packs/cg_firefly)
  --palette NAME   LUT in DIR/luts (default firefly-neutral)
  --sky N          skybox still N instead of a random one from the pool
  --hold           stay connected, blocked on X events, instead of exiting";

struct Args {
    pack: PathBuf,
    palette: String,
    sky: Option<u32>,
    hold: bool,
}

struct Pack {
    width: usize,
    height: usize,
    scale: usize,
    layers: Vec<Layer>,
}

struct Layer {
    name: String,
    choices: Vec<PathBuf>,
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
    let start = Instant::now();
    let pack = load_pack(&args.pack)?;
    let lut_path = args.pack.join("luts").join(format!("{}.bin", args.palette));
    let lut = fs::read(&lut_path)
        .map_err(|e| format!("palette {:?}: {e} (available: {})", args.palette, palettes(&args.pack)))?;
    if lut.len() != 256 * 4 {
        return Err(format!("{}: {} bytes, expected 1024", lut_path.display(), lut.len()).into());
    }

    let mut frame = vec![0u8; pack.width * pack.height];
    for layer in &pack.layers {
        let path = &layer.choices[choose(layer, args.sky)?];
        let indices = read_indexed(path, pack.width, pack.height)?;
        for (dst, &src) in frame.iter_mut().zip(&indices) {
            if src != 0 {
                *dst = src;
            }
        }
        println!("layer {}: {}", layer.name, path.display());
    }
    let composited = start.elapsed();

    let (width, height) = (pack.width * pack.scale, pack.height * pack.scale);
    let bgra = render(&frame, pack.width, pack.scale, &lut);
    let rendered = start.elapsed();

    let (conn, screen_num) = x11rb::connect(None)?;
    let pixmap = set_root_background(&conn, screen_num, &bgra, u16::try_from(width)?, u16::try_from(height)?)?;
    let uploaded = start.elapsed();
    println!(
        "root pixmap 0x{pixmap:x}, {width}x{height}: load+composite {:.1} ms, render {:.1} ms, upload {:.1} ms",
        ms(composited),
        ms(rendered - composited),
        ms(uploaded - rendered)
    );

    if args.hold {
        drop((frame, bgra, lut)); // so RSS while holding reflects an idle engine
        loop {
            if let Err(e) = conn.wait_for_event() {
                println!("X connection closed ({e}); another background setter took over");
                return Ok(());
            }
        }
    }
    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut args = Args { pack: "rip/packs/cg_firefly".into(), palette: "firefly-neutral".into(), sky: None, hold: false };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--pack" => args.pack = it.next().ok_or(USAGE)?.into(),
            "--palette" => args.palette = it.next().ok_or(USAGE)?,
            "--sky" => args.sky = Some(it.next().ok_or(USAGE)?.parse()?),
            "--hold" => args.hold = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {arg:?}\n{USAGE}").into()),
        }
    }
    Ok(args)
}

fn load_pack(dir: &Path) -> Result<Pack> {
    let path = dir.join("scene.txt");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e} (run tools/pack.py)", path.display()))?;
    let (mut size, mut scale, mut layers) = (None, None, Vec::new());
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let bad = || format!("{}: bad line {line:?}", path.display());
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("size") => {
                let mut dim = || fields.next().and_then(|v| v.parse::<usize>().ok()).ok_or_else(bad);
                size = Some((dim()?, dim()?));
            }
            Some("scale") => scale = Some(fields.next().and_then(|v| v.parse::<usize>().ok()).ok_or_else(bad)?),
            Some("layer") => {
                let name = fields.next().ok_or_else(bad)?.to_string();
                let choices: Vec<PathBuf> = fields.map(|f| dir.join(f)).collect();
                if choices.is_empty() {
                    return Err(bad().into());
                }
                layers.push(Layer { name, choices });
            }
            _ => return Err(bad().into()),
        }
    }
    let ((width, height), scale) = size.zip(scale).ok_or_else(|| format!("{}: needs size and scale", path.display()))?;
    Ok(Pack { width, height, scale, layers })
}

/// A random image of the layer, or with `--sky N` the sky layer's still N.
fn choose(layer: &Layer, sky: Option<u32>) -> Result<usize> {
    if let (Some(n), "sky") = (sky, layer.name.as_str()) {
        let stem = n.to_string();
        return layer
            .choices
            .iter()
            .position(|p| p.file_stem() == Some(OsStr::new(&stem)))
            .ok_or_else(|| format!("sky layer has no still {n}").into());
    }
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.subsec_nanos() as usize;
    Ok(nanos % layer.choices.len())
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

/// Indices -> BGRX through the LUT, each native pixel expanded to scale x scale.
fn render(frame: &[u8], width: usize, scale: usize, lut: &[u8]) -> Vec<u8> {
    let row_bytes = width * scale * 4;
    let mut out = vec![0u8; frame.len() * scale * scale * 4];
    for (row, band) in frame.chunks_exact(width).zip(out.chunks_exact_mut(row_bytes * scale)) {
        let (first, rest) = band.split_at_mut(row_bytes);
        for (&index, block) in row.iter().zip(first.chunks_exact_mut(4 * scale)) {
            let colour = &lut[index as usize * 4..][..4];
            for px in block.chunks_exact_mut(4) {
                px.copy_from_slice(colour);
            }
        }
        for line in rest.chunks_exact_mut(row_bytes) {
            line.copy_from_slice(first);
        }
    }
    out
}

/// Upload into a new root-depth pixmap, make it the root background and advertise it in _XROOTPMAP_ID and
/// ESETROOT_PMAP_ID. Like feh, free the previous setter's retained pixmap first and retain ours on exit.
fn set_root_background(conn: &impl Connection, screen_num: usize, bgra: &[u8], width: u16, height: u16) -> Result<u32> {
    let setup = conn.setup();
    let screen = &setup.roots[screen_num];
    let (root, depth) = (screen.root, screen.root_depth);
    if (screen.width_in_pixels, screen.height_in_pixels) != (width, height) {
        eprintln!("warning: screen is {}x{}, the scene is {width}x{height}", screen.width_in_pixels, screen.height_in_pixels);
    }
    let bits_per_pixel = setup.pixmap_formats.iter().find(|f| f.depth == depth).map(|f| f.bits_per_pixel);
    let masks = screen
        .allowed_depths
        .iter()
        .flat_map(|d| &d.visuals)
        .find(|v| v.visual_id == screen.root_visual)
        .map(|v| (v.red_mask, v.green_mask, v.blue_mask));
    if !matches!(depth, 24 | 32)
        || bits_per_pixel != Some(32)
        || setup.image_byte_order != ImageOrder::LSB_FIRST
        || masks != Some((0xff0000, 0xff00, 0xff))
    {
        return Err("root visual is not 24-bit BGRX with 32 bpp, the only format supported".into());
    }

    let atom = |name: &[u8]| -> Result<u32> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let (xrootpmap, esetroot) = (atom(b"_XROOTPMAP_ID")?, atom(b"ESETROOT_PMAP_ID")?);
    let old = conn.get_property(false, root, esetroot, AtomEnum::PIXMAP, 0, 1)?.reply()?;
    if let Some(id) = old.value32().and_then(|mut v| v.next()) {
        conn.kill_client(id)?.ignore_error();
    }

    let pixmap = conn.generate_id()?;
    conn.create_pixmap(depth, pixmap, root, width, height)?.check()?;
    let gc = conn.generate_id()?;
    conn.create_gc(gc, pixmap, &CreateGCAux::new())?.check()?;
    let row_bytes = usize::from(width) * 4;
    let rows = ((conn.maximum_request_bytes() - 32) / row_bytes).clamp(1, usize::from(height));
    for (i, chunk) in bgra.chunks(rows * row_bytes).enumerate() {
        let (h, y) = (u16::try_from(chunk.len() / row_bytes)?, i16::try_from(i * rows)?);
        conn.put_image(ImageFormat::Z_PIXMAP, pixmap, gc, width, h, 0, y, 0, depth, chunk)?.check()?;
    }
    conn.free_gc(gc)?;

    conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().background_pixmap(pixmap))?.check()?;
    conn.clear_area(false, root, 0, 0, 0, 0)?;
    conn.change_property32(PropMode::REPLACE, root, xrootpmap, AtomEnum::PIXMAP, &[pixmap])?;
    conn.change_property32(PropMode::REPLACE, root, esetroot, AtomEnum::PIXMAP, &[pixmap])?;
    // The pixmap outlives this process, so _XROOTPMAP_ID never dangles; the next setter frees it.
    conn.set_close_down_mode(CloseDown::RETAIN_PERMANENT)?.check()?;
    Ok(pixmap)
}

fn palettes(pack: &Path) -> String {
    let mut names: Vec<String> = fs::read_dir(pack.join("luts"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    names.sort();
    names.join(", ")
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
