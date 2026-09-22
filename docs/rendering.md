# Rendering

The engine keeps a composited index canvas at native size. When a timeline step or a drift step changes part of
it, only that part is recomposited, converted through the palette lookup table into BGRX, and uploaded. The X
server scales it onto the screen. `output.rs` holds the X side, `render.rs` the conversion, `frame.rs` the
merging of dirty rectangles.

## Two output targets

- **A desktop window** (`--output window`): a screen-sized override-redirect window of type
  `_NET_WM_WINDOW_TYPE_DESKTOP` at the bottom of the stack. Drawing into it damages exactly the changed
  rectangles, which the compositor recomposites. This is what `auto` picks whenever a compositor owns
  `_NET_WM_CM_S0`.
- **The root background** (`--output root`): a root-depth pixmap set as the root window's background and
  advertised in `_XROOTPMAP_ID` and `ESETROOT_PMAP_ID` the way feh does it, with the changed areas cleared on the
  root window so it repaints from the pixmap. For X without a compositor, and for `--once`. The engine frees the
  previous setter's retained pixmap, like feh, and retains its own on exit, so the property never dangles.

The window is needed under a compositor because picom reads the root pixmap only when `_XROOTPMAP_ID` changes.
Drawing into the pixmap afterwards never reaches the screen.

Under i3 the wallpaper is started with `--output window` rather than `auto`, because picom may not own the
screen yet at the moment i3 runs its `exec` lines.

## Upload and scaling

Xorg with the glamor acceleration turns every image upload into a GPU texture upload. Scaling on the CPU and
uploading screen-sized rectangles cost about 6 % of a core in the engine and 11 % in Xorg for a busy scene. So
the engine uploads changed canvas pixels at native size through MIT-SHM into a canvas-sized pixmap, and a RENDER
transform with the nearest filter scales that picture onto the target. The result is pixel-identical to CPU
scaling. `geometry.rs` uses the same mapping as the transform (each output pixel samples the canvas at its
centre) to know which screen rectangles a canvas rectangle covers.

Uploads go through one shared memory segment; the engine syncs with the server before overwriting memory the
server may still be reading.

## Atomic frames

All uploads of a frame land in the hidden source pixmap first. Then every composite of the frame runs under a
server grab, so a compositor never presents half a frame. The on-screen cadence of 0.1 s steps measures as
exactly 100 ms.

## Draw limits

- **Merging.** Dirty rectangles whose union costs no more pixels than drawing them apart are merged. Past
  `--max-rects` (16) draws, each separate upload and scaled composite costs more than a few extra pixels, so the
  dirty area is covered by that many horizontal strips instead.
- **Drift frames** may use up to 256 draws. Their changes are thin lines spread over the whole canvas: merged into
  16 strips they would damage 40 to 90 % of the screen, uncapped 9 to 31 %, for about a millisecond of X time
  once a second.
- **Frame cap.** Holds shorter than 1/`--max-fps` are stretched. The default of 20 fps lets the game's 0.05 s
  animations (`mini_cg_momp`, `cg_eyelash`) keep their speed.

## What it costs

Measured as process CPU on an empty workspace, on a laptop with Intel integrated graphics, percent of one core
(baseline with a static desktop: picom 0.6 %, Xorg 1.4 %):

| Scene | Engine | picom | Xorg |
|---|---|---|---|
| `cg_floor` (blinks only) | 0.0 | | |
| `mini_cg_run` (10 fps, most of the window changes) | 0.7 | 2.5 | 4.2 |
| `cg_fall_far` (10 fps, large frames over the sky) | 2.1 | 2.5 | 5.0 |
| `mini_cg_momp` (20 fps) | 1.2 | 4.1 | 6.2 |
| drift alone (`cg_firefly`, `cg_floor`) | 0.2 to 0.3 | | |

Resident memory is 4 to 9 MB. While the desktop is visible, picom and Xorg cost more than the engine, and a
compositor recomposites every damaged region, so keeping the damage small matters more than any other
optimisation. Pausing whenever the wallpaper cannot be seen is what keeps the everyday cost near zero; see
[pause.md](pause.md). Large-area animations still cost a few percent in picom and Xorg while visible; a lower
frame rate for those scenes is the obvious lever if that ever matters.

## Startup

`molokolive` loads a pack, composites and renders the first frame in a few milliseconds and uploads it, then
publishes: maps the window at the bottom of the stack (or sets the root pixmap), and only then ends the previous
molokolive, so nothing flashes.
