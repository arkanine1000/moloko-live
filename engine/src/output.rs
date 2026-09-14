//! Where frames go. Changed canvas pixels are uploaded at native size through a MIT-SHM segment into a canvas-sized
//! source pixmap; the X server scales them onto the target with a RENDER transform and the nearest filter (on this
//! machine glamor, so on the GPU). Two targets:
//!
//! - window: a screen-sized override-redirect window of type _NET_WM_WINDOW_TYPE_DESKTOP at the bottom of the
//!   stack. Drawing into it damages exactly the changed rectangles, which a compositor recomposites. The default
//!   whenever a compositor is running: picom reads the root pixmap only when _XROOTPMAP_ID changes, so updates
//!   drawn into it never reach the screen.
//! - root: a root-depth pixmap set as the root background and advertised in _XROOTPMAP_ID/ESETROOT_PMAP_ID the
//!   way feh does it, with changed areas cleared on the root window. For X without a compositor.

use std::os::fd::AsFd;
use std::ptr;
use std::time::Duration;

use rustix::event::{PollFd, PollFlags, Timespec};
use rustix::fs::MemfdFlags;
use rustix::mm::{MapFlags, ProtFlags};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::render::{self, ConnectionExt as _, CreatePictureAux, PictOp};
use x11rb::protocol::shm::ConnectionExt as _;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, CloseDown, ConfigureWindowAux, ConnectionExt as _, CreateGCAux,
    CreateWindowAux, EventMask, ImageFormat, ImageOrder, PropMode, Rectangle, StackMode, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::Result;
use crate::pack::Rect;
use crate::render::Geometry;

const CLASS: &[u8] = b"molokolive\0molokolive\0";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// window if a compositor owns _NET_WM_CM_S<screen>, else root
    Auto,
    Root,
    Window,
}

pub struct Output {
    conn: RustConnection,
    root: u32,
    depth: u8,
    format: render::Pictformat,
    /// The window, or the root background pixmap.
    drawable: u32,
    window: bool,
    target: render::Picture,
    gc: u32,
    source: Option<Source>,
    pub width: usize,
    pub height: usize,
}

/// The canvas on the server, and the shared memory its updates are uploaded through.
struct Source {
    pixmap: u32,
    picture: render::Picture,
    segment: u32,
    memory: *mut u8,
    capacity: usize,
}

impl Output {
    /// Connect and create the target. Call `prepare` for the canvas; nothing is visible until `publish`.
    pub fn connect(target: Target) -> Result<Output> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let setup = conn.setup();
        let screen = &setup.roots[screen_num];
        let (root, depth) = (screen.root, screen.root_depth);
        let (width, height) = (screen.width_in_pixels, screen.height_in_pixels);
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
        let shm = conn.shm_query_version()?.reply().map_err(|e| format!("MIT-SHM unavailable: {e}"))?;
        if (shm.major_version, shm.minor_version) < (1, 2) {
            return Err("MIT-SHM 1.2 (fd passing) is required".into());
        }
        let rendering = conn.render_query_version(0, 11)?.reply().map_err(|e| format!("RENDER unavailable: {e}"))?;
        if (rendering.major_version, rendering.minor_version) < (0, 6) {
            return Err("RENDER 0.6 (picture transforms) is required".into());
        }
        let formats = conn.render_query_pict_formats()?.reply()?;
        let format = formats
            .screens
            .get(screen_num)
            .into_iter()
            .flat_map(|s| &s.depths)
            .flat_map(|d| &d.visuals)
            .find(|v| v.visual == screen.root_visual)
            .map(|v| v.format)
            .ok_or("no RENDER format for the root visual")?;

        let window = match target {
            Target::Root => false,
            Target::Window => true,
            Target::Auto => {
                let cm = conn.intern_atom(false, format!("_NET_WM_CM_S{screen_num}").as_bytes())?.reply()?.atom;
                conn.get_selection_owner(cm)?.reply()?.owner != x11rb::NONE
            }
        };
        let drawable = conn.generate_id()?;
        if window {
            let attributes = CreateWindowAux::new()
                .override_redirect(1)
                .background_pixmap(x11rb::NONE)
                .event_mask(EventMask::EXPOSURE);
            conn.create_window(0, drawable, root, 0, 0, width, height, 0, WindowClass::INPUT_OUTPUT, 0, &attributes)?
                .check()?;
            let atom = |name: &[u8]| -> Result<u32> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
            let (wm_type, desktop) = (atom(b"_NET_WM_WINDOW_TYPE")?, atom(b"_NET_WM_WINDOW_TYPE_DESKTOP")?);
            conn.change_property32(PropMode::REPLACE, drawable, wm_type, AtomEnum::ATOM, &[desktop])?;
            conn.change_property8(PropMode::REPLACE, drawable, AtomEnum::WM_CLASS, AtomEnum::STRING, CLASS)?;
            conn.change_property8(PropMode::REPLACE, drawable, AtomEnum::WM_NAME, AtomEnum::STRING, b"molokolive")?;
        } else {
            conn.create_pixmap(depth, drawable, root, width, height)?.check()?;
        }
        let target = conn.generate_id()?;
        conn.render_create_picture(target, drawable, format, &CreatePictureAux::new())?.check()?;
        let gc = conn.generate_id()?;
        conn.create_gc(gc, root, &CreateGCAux::new().graphics_exposures(0))?.check()?;
        Ok(Output {
            conn,
            root,
            depth,
            format,
            drawable,
            window,
            target,
            gc,
            source: None,
            width: width.into(),
            height: height.into(),
        })
    }

    pub fn kind(&self) -> &'static str {
        if self.window { "desktop window" } else { "root background" }
    }

    /// Create the canvas-sized source picture, scaled onto the screen as `geometry` maps it, and its shared memory.
    pub fn prepare(&mut self, canvas_width: usize, canvas_height: usize, geometry: &Geometry) -> Result<()> {
        let conn = &self.conn;
        let (w, h) = (u16::try_from(canvas_width)?, u16::try_from(canvas_height)?);
        let pixmap = conn.generate_id()?;
        conn.create_pixmap(self.depth, pixmap, self.root, w, h)?.check()?;
        let picture = conn.generate_id()?;
        conn.render_create_picture(picture, pixmap, self.format, &CreatePictureAux::new())?.check()?;
        // Screen -> canvas: (x - offset) / scale. RENDER samples each destination pixel at its centre, matching
        // Geometry's maps.
        let fixed = |v: f64| (v * 65536.0).round() as render::Fixed;
        let inverse = 1.0 / geometry.scale;
        let transform = render::Transform {
            matrix11: fixed(inverse),
            matrix12: 0,
            matrix13: fixed(-geometry.offset.0 * inverse),
            matrix21: 0,
            matrix22: fixed(inverse),
            matrix23: fixed(-geometry.offset.1 * inverse),
            matrix31: 0,
            matrix32: 0,
            matrix33: fixed(1.0),
        };
        conn.render_set_picture_transform(picture, transform)?.check()?;
        conn.render_set_picture_filter(picture, b"nearest", &[])?.check()?;

        let capacity = canvas_width * canvas_height * 4;
        let fd = rustix::fs::memfd_create("molokolive", MemfdFlags::CLOEXEC)?;
        rustix::fs::ftruncate(&fd, capacity as u64)?;
        // SAFETY: a fresh shared mapping of a memfd we sized; it stays mapped for the life of the process, and only
        // this Output hands out slices of it.
        let memory = unsafe {
            rustix::mm::mmap(ptr::null_mut(), capacity, ProtFlags::READ | ProtFlags::WRITE, MapFlags::SHARED, &fd, 0)?
        } as *mut u8;
        let segment = conn.generate_id()?;
        conn.shm_attach_fd(segment, fd, true)?.check()?;
        self.source = Some(Source { pixmap, picture, segment, memory, capacity });
        Ok(())
    }

    fn source(&self) -> &Source {
        self.source.as_ref().expect("Output::prepare not called")
    }

    /// Bytes available in the shared image.
    pub fn capacity(&self) -> usize {
        self.source().capacity
    }

    /// `len` bytes of the shared image from `offset`, to convert canvas pixels into before `upload`.
    pub fn image(&mut self, offset: usize, len: usize) -> &mut [u8] {
        let source = self.source();
        assert!(offset + len <= source.capacity);
        // SAFETY: in bounds of the mapping, and &mut self keeps the slice exclusive.
        unsafe { std::slice::from_raw_parts_mut(source.memory.add(offset), len) }
    }

    /// Copy BGRX rows for canvas rectangle `r`, stored in the shared image at `offset`, into the source pixmap. The
    /// server reads the memory when it handles the request: `sync` before overwriting that part of the image.
    pub fn upload(&self, r: Rect, offset: usize) -> Result<()> {
        let source = self.source();
        let (w, h) = (u16::try_from(r.width())?, u16::try_from(r.height())?);
        let (x, y) = (i16::try_from(r.x0)?, i16::try_from(r.y0)?);
        let format = ImageFormat::Z_PIXMAP.into();
        let offset = u32::try_from(offset)?;
        self.conn.shm_put_image(source.pixmap, self.gc, w, h, 0, 0, w, h, x, y, self.depth, format, false, source.segment, offset)?;
        Ok(())
    }

    /// Scale the source onto screen rectangle `r` and make it visible.
    pub fn show(&self, r: Rect) -> Result<()> {
        let (x, y) = (i16::try_from(r.x0)?, i16::try_from(r.y0)?);
        let (w, h) = (u16::try_from(r.width())?, u16::try_from(r.height())?);
        self.conn.render_composite(PictOp::SRC, self.source().picture, x11rb::NONE, self.target, x, y, 0, 0, x, y, w, h)?;
        self.expose_root(r)
    }

    /// Scale the source onto every screen rectangle of one frame under a server grab: a compositor's damage
    /// requests wait until the whole frame is drawn, so it never presents half of one.
    pub fn show_frame(&self, rects: &[Rect]) -> Result<()> {
        self.conn.grab_server()?;
        let shown = rects.iter().try_for_each(|&r| self.show(r));
        self.conn.ungrab_server()?;
        shown
    }

    /// Fill screen rectangles with a BGRX colour.
    pub fn fill(&self, rects: &[Rect], bgrx: [u8; 4]) -> Result<()> {
        if rects.is_empty() {
            return Ok(());
        }
        let channel = |v: u8| u16::from(v) * 257;
        let color = render::Color { red: channel(bgrx[2]), green: channel(bgrx[1]), blue: channel(bgrx[0]), alpha: 0xffff };
        let rectangles = rects
            .iter()
            .map(|r| Ok(Rectangle { x: i16::try_from(r.x0)?, y: i16::try_from(r.y0)?, width: u16::try_from(r.width())?, height: u16::try_from(r.height())? }))
            .collect::<Result<Vec<_>>>()?;
        self.conn.render_fill_rectangles(PictOp::SRC, self.target, color, &rectangles)?;
        rects.iter().try_for_each(|&r| self.expose_root(r))
    }

    /// The root window repaints from its background pixmap only where cleared; a window was damaged by the drawing.
    fn expose_root(&self, r: Rect) -> Result<()> {
        if !self.window {
            let (x, y) = (i16::try_from(r.x0)?, i16::try_from(r.y0)?);
            self.conn.clear_area(false, self.root, x, y, u16::try_from(r.width())?, u16::try_from(r.height())?)?;
        }
        Ok(())
    }

    /// Paint the screen from the uploaded canvas: `visible` scaled from the source, `letterbox` in `background`.
    pub fn paint(&self, visible: Option<Rect>, letterbox: &[Rect], background: [u8; 4]) -> Result<()> {
        self.fill(letterbox, background)?;
        visible.map_or(Ok(()), |r| self.show(r))
    }

    /// Show the first frame (already uploaded) and replace a running molokolive. The window is mapped at the
    /// bottom of the stack before the old one goes, so nothing flashes. The root background frees the previous
    /// setter's retained pixmap like feh and retains its own on exit, so _XROOTPMAP_ID never dangles.
    pub fn publish(&self, visible: Option<Rect>, letterbox: &[Rect], background: [u8; 4]) -> Result<()> {
        let atom = |name: &[u8]| -> Result<u32> { Ok(self.conn.intern_atom(false, name)?.reply()?.atom) };
        let instance = atom(b"_MOLOKOLIVE_WINDOW")?;
        let previous = self.conn.get_property(false, self.root, instance, AtomEnum::WINDOW, 0, 1)?.reply()?;
        let previous = previous.value32().and_then(|mut v| v.next()).filter(|&w| w != self.drawable && self.is_ours(w));

        if self.window {
            self.conn.map_window(self.drawable)?;
            self.conn.configure_window(self.drawable, &ConfigureWindowAux::new().stack_mode(StackMode::BELOW))?;
            self.sync()?;
            self.paint(visible, letterbox, background)?;
            self.conn.change_property32(PropMode::REPLACE, self.root, instance, AtomEnum::WINDOW, &[self.drawable])?;
        } else {
            self.paint(visible, letterbox, background)?;
            let (xrootpmap, esetroot) = (atom(b"_XROOTPMAP_ID")?, atom(b"ESETROOT_PMAP_ID")?);
            let old = self.conn.get_property(false, self.root, esetroot, AtomEnum::PIXMAP, 0, 1)?.reply()?;
            if let Some(id) = old.value32().and_then(|mut v| v.next()) {
                self.conn.kill_client(id)?.ignore_error();
            }
            let attributes = ChangeWindowAttributesAux::new().background_pixmap(self.drawable);
            self.conn.change_window_attributes(self.root, &attributes)?.check()?;
            self.conn.clear_area(false, self.root, 0, 0, 0, 0)?;
            self.conn.change_property32(PropMode::REPLACE, self.root, xrootpmap, AtomEnum::PIXMAP, &[self.drawable])?;
            self.conn.change_property32(PropMode::REPLACE, self.root, esetroot, AtomEnum::PIXMAP, &[self.drawable])?;
            self.conn.delete_property(self.root, instance)?;
            self.conn.set_close_down_mode(CloseDown::RETAIN_PERMANENT)?.check()?;
        }
        if let Some(old) = previous {
            self.conn.kill_client(old)?.ignore_error();
        }
        self.sync()
    }

    /// Whether `window` still exists and is a molokolive window; its id may since belong to another client.
    fn is_ours(&self, window: u32) -> bool {
        let Ok(cookie) = self.conn.get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 16) else {
            return false;
        };
        cookie.reply().is_ok_and(|r| r.value == CLASS)
    }

    /// Flush and wait for the server to handle everything sent, so the shared image may be overwritten.
    pub fn sync(&self) -> Result<()> {
        self.conn.get_input_focus()?.reply()?;
        Ok(())
    }

    /// Handle queued X events -> screen areas to repaint. Errors from unchecked requests fail.
    pub fn drain_events(&self) -> Result<Vec<Rect>> {
        let mut exposed = Vec::new();
        while let Some(event) = self.conn.poll_for_event()? {
            match event {
                Event::Error(e) => return Err(format!("X error: {e:?}").into()),
                Event::Expose(e) if e.window == self.drawable => {
                    let r = Rect {
                        x0: usize::from(e.x),
                        y0: usize::from(e.y),
                        x1: usize::from(e.x) + usize::from(e.width),
                        y1: usize::from(e.y) + usize::from(e.height),
                    };
                    exposed.extend(r.intersect(&Rect { x0: 0, y0: 0, x1: self.width, y1: self.height }));
                }
                _ => {}
            }
        }
        Ok(exposed)
    }

    /// Block until the X connection is readable or `timeout` passes (None: no timeout).
    pub fn wait(&self, timeout: Option<Duration>) -> Result<()> {
        let timespec = timeout.map(|d| Timespec { tv_sec: d.as_secs() as _, tv_nsec: d.subsec_nanos() as _ });
        let fd = self.conn.stream().as_fd();
        let mut fds = [PollFd::new(&fd, PollFlags::IN)];
        match rustix::event::poll(&mut fds, timespec.as_ref()) {
            Ok(_) | Err(rustix::io::Errno::INTR) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
